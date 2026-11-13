//! Chat child tests: subagents created from an Agent chat, their report turn on
//! the creator tab, read/merge/archive/message_child, waves, removal with the
//! parent, and the settled rule.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::TurnRequest;
use recur_agent_tools::CreateWorkspaceRequest;
use recur_agent_tools::names::mcp;
use recur_domain::{
    Agent, CHAT_MODE_AGENT, CHAT_MODE_LOOP, ChildFanOut, FEED_CARD_FAN_OUT, Finding, FindingStatus,
    Plan, PlanStep, Severity, UserQuestion, Workspace, WorkspaceState,
};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::test_support::*;
use crate::chat::children::{
    CHILD_OUTCOME_CLOSED, CHILD_OUTCOME_MERGED, CHILD_OUTCOME_REPORT_PENDING,
    CHILD_OUTCOME_SETTLED, PARENT_STEER_PREFIX,
};
use crate::chat::create_tool::want_child;
use crate::codes::{PAIR_AGENT_CODE, chat_turn_key, is_chat_child};
use crate::loops::test_support::CaptureScript;
use crate::testing::wait_for;
use crate::turn::finalize::CHAT_INJECTED_SENTINEL;

/// Waits for every tracked task, then reopens the tracker.
async fn drain(t: &TaskTracker) {
    t.close();
    t.wait().await;
    t.reopen();
}

fn child_runner(reply: &'static str) -> Arc<CaptureScript> {
    capture_same("child", reply)
}

async fn parent(fx: &Fx) -> Workspace {
    let caller = fx.workspace("parent").await;
    fx.eng().ensure_pair_agent(&caller.id, PAIR_AGENT_CODE, "");
    caller
}

fn by_goal(fx: &Fx, title: &str) -> Workspace {
    fx.te
        .store
        .workspaces()
        .into_iter()
        .rfind(|w| w.goal == title)
        .unwrap_or_else(|| panic!("workspace {title} not found"))
}

async fn create(fx: &Fx, caller: &Workspace, title: &str) -> String {
    fx.eng()
        .create_workspace_from_chat(
            &caller.id,
            "PA2",
            CreateWorkspaceRequest {
                title: title.into(),
                prompt: "Do it.".into(),
                ..CreateWorkspaceRequest::default()
            },
        )
        .await
}

/// Creates a child of `caller` via the tool, waits for its first turn, commits
/// helper.txt in it, and returns it settled.
async fn spawn_child(fx: &Fx, caller: &Workspace, title: &str) -> Workspace {
    let reply = create(fx, caller, title).await;
    assert!(reply.starts_with("Created subagent "), "reply = {reply}");
    drain(&fx.eng().tasks.schedules).await;
    let child = by_goal(fx, title);
    assert!(
        child.parent_id == caller.id
            && is_chat_child(&child)
            && !child.worktree_path.is_empty()
            && child.fan_out.as_ref().is_some_and(|f| f.chat == "PA2"),
        "child = {child:?}"
    );
    std::fs::write(
        std::path::Path::new(&child.worktree_path).join("helper.txt"),
        "child\n",
    )
    .unwrap();
    fx.eng()
        .git_manager(&fx.repo.path)
        .commit(&child.worktree_path, "child work", "t", "t@x")
        .await
        .unwrap();
    assert!(
        fx.eng().child_settled_for_chat(&child.id),
        "child after its first turn must count as settled: state={:?} pending={}",
        child.state,
        child.scheduled_prompt_pending
    );
    child
}

fn turn_in(c: &CaptureScript, worktree: &str) -> Option<TurnRequest> {
    c.specs
        .lock()
        .iter()
        .rfind(|s| s.worktree.to_string_lossy() == worktree)
        .cloned()
}

fn allows(req: &TurnRequest, tool: &str) -> bool {
    has_tool(req, tool)
}

const FAN_OUT_TOOLS: [&str; 7] = [
    mcp::ASK_USER,
    mcp::OPEN_PR,
    mcp::CREATE_WORKSPACE,
    mcp::MERGE_CHILD,
    mcp::READ_CHILD,
    mcp::ARCHIVE_CHILD,
    mcp::MESSAGE_CHILD,
];

fn outcome(fx: &Fx, id: &str) -> String {
    fx.te
        .store
        .workspace(id)
        .and_then(|w| w.fan_out)
        .map(|f| f.outcome)
        .unwrap_or_default()
}

async fn wait_report(fx: &Fx, caller: &Workspace, child: &Workspace, n: usize, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if outcome(fx, &child.id) == CHILD_OUTCOME_SETTLED
            && fx.eng().notice_count(&caller.id, "PA2", child).await == n
        {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn report_turn(c: &CaptureScript, caller: &Workspace, child: &Workspace) -> TurnRequest {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let got = c
            .specs
            .lock()
            .iter()
            .rfind(|s| {
                s.worktree.to_string_lossy() == caller.worktree_path && s.prompt.contains(&child.id)
            })
            .cloned();
        if let Some(r) = got {
            return r;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the creator tab never received the child report turn"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_spawned_child_executes_its_prompt_locally() {
    let c = child_runner("Done.");
    let fx = Fx::new(Some(c.provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Implement the feature").await;
    let spec = turn_in(&c, &child.worktree_path).expect("child first turn not captured");
    assert!(!spec.append_system_prompt.is_empty());
    assert!(spec.chat_child, "child first turn should set chat_child");
    let p = &spec.append_system_prompt;
    assert!(!p.contains("beyond a very simple task"), "{p}");
    assert!(p.contains("execute the assigned task here"), "{p}");
    assert!(!p.contains(mcp::CREATE_WORKSPACE), "{p}");
    assert!(p.contains(&format!("Do not call {}", mcp::ASK_USER)), "{p}");
    assert!(
        allows(&spec, mcp::RENAME_BRANCH),
        "{:?}",
        spec.allowed_tools
    );
    for tl in FAN_OUT_TOOLS {
        assert!(!allows(&spec, tl), "child must not allow {tl}");
    }
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_standalone_workspace_keeps_the_fan_out_tools() {
    let c = capture_same("solo", "Done.");
    let fx = Fx::new(Some(c.provider()));
    let caller = parent(&fx).await;
    let reply = fx
        .eng()
        .create_workspace_from_chat(
            &caller.id,
            "PA2",
            CreateWorkspaceRequest {
                title: "Independent".into(),
                prompt: "Do it.".into(),
                standalone: Some(true),
                ..CreateWorkspaceRequest::default()
            },
        )
        .await;
    assert!(reply.starts_with("Created workspace "), "reply = {reply}");
    drain(&fx.eng().tasks.schedules).await;
    let spawned = by_goal(&fx, "Independent");
    assert!(spawned.parent_id.is_empty() && !spawned.worktree_path.is_empty());
    let spec = turn_in(&c, &spawned.worktree_path).expect("standalone first turn");
    assert!(!spec.chat_child);
    for tl in FAN_OUT_TOOLS {
        assert!(allows(&spec, tl), "standalone missing {tl}");
    }
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_reports_to_its_parent_then_merges_on_request() {
    let c = child_runner("Done.");
    let fx = Fx::new(Some(c.provider()));
    fx.eng().set_fan_out_poll(Duration::from_millis(50));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Add helper").await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let report = loop {
        let got = c
            .specs
            .lock()
            .iter()
            .rfind(|s| {
                s.worktree.to_string_lossy() == caller.worktree_path
                    && (s.chat_code == "PA2" || s.agent_id.ends_with("-PA2"))
                    && s.prompt.contains(&child.id)
            })
            .cloned();
        if let Some(r) = got {
            break r;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "PA2 never got the report"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    for want in [
        "Done.".to_owned(),
        "committed".into(),
        format!("read_child {}", child.id),
        format!("merge_child {}", child.id),
        format!("archive_child {}", child.id),
    ] {
        assert!(
            report.prompt.contains(&want),
            "notice missing {want}:\n{}",
            report.prompt
        );
    }
    assert!(
        !report.prompt.contains("helper.txt") && report.prompt.len() <= 900,
        "notice should be brief ({} chars):\n{}",
        report.prompt.len(),
        report.prompt
    );
    let (st, id) = (Arc::clone(&fx.te.store), child.id.clone());
    wait_for(Duration::from_secs(5), "child outcome settled", || {
        st.workspace(&id)
            .and_then(|w| w.fan_out)
            .is_some_and(|f| f.outcome == CHILD_OUTCOME_SETTLED)
    })
    .await;

    let git = fx.eng().git_manager(&fx.repo.path);
    std::fs::write(
        std::path::Path::new(&caller.worktree_path).join("wip.txt"),
        "wip\n",
    )
    .unwrap();
    let msg = fx.eng().merge_child_from_chat(&caller.id, &child.id).await;
    assert!(msg.contains("uncommitted"), "dirty parent merge = {msg}");
    git.commit(&caller.worktree_path, "parent wip", "t", "t@x")
        .await
        .unwrap();
    let msg = fx.eng().merge_child_from_chat(&caller.id, &child.id).await;
    assert!(
        msg.starts_with("Merged subagent"),
        "clean parent merge = {msg}"
    );
    assert!(
        std::path::Path::new(&caller.worktree_path)
            .join("helper.txt")
            .exists()
    );
    assert!(
        recur_git::status_map(&caller.worktree_path)
            .await
            .unwrap_or_default()
            .is_empty(),
        "parent worktree dirty after merge"
    );
    if let Some(after) = fx.te.store.workspace(&child.id) {
        assert!(after.archived && outcome(&fx, &child.id) == CHILD_OUTCOME_MERGED);
    }
    let msg = fx.eng().merge_child_from_chat(&caller.id, &child.id).await;
    assert!(msg.contains("not a live child"), "second merge = {msg}");
    drain(&fx.eng().tasks.teardown).await;
    assert!(!git.base_branch_exists(&child.branch).await);
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_child_and_archive_without_merging() {
    let fx = Fx::new(Some(child_runner("Research says X.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Research").await;
    let eng = fx.eng();
    let report = eng.read_child_from_chat(&caller.id, &child.id).await;
    for want in ["Research says X.", "helper.txt", "```diff", "+child"] {
        assert!(
            report.contains(want),
            "read_child missing {want}:\n{report}"
        );
    }
    let own = eng.read_child_from_chat(&caller.id, &caller.id).await;
    assert!(own.contains("not a child"), "{own}");
    assert!(
        eng.read_child_artifact_from_chat(&caller.id, "nope", "a1")
            .is_err()
    );
    let msg = eng.archive_child_from_chat(&caller.id, &child.id).await;
    assert!(
        msg.starts_with("Archived subagent") && msg.contains("discarding 1 unmerged commit"),
        "archive = {msg}"
    );
    drain(&eng.tasks.teardown).await;
    assert!(
        !eng.git_manager(&fx.repo.path)
            .base_branch_exists(&child.branch)
            .await
    );
    let after = fx.te.store.workspace(&child.id).expect("archived row");
    assert!(after.archived && after.state == WorkspaceState::Stopped);
    assert_eq!(outcome(&fx, &child.id), CHILD_OUTCOME_CLOSED);
    assert!(
        !std::path::Path::new(&caller.worktree_path)
            .join("helper.txt")
            .exists()
    );
    assert!(
        eng.read_child_from_chat(&caller.id, &child.id)
            .await
            .contains("archived")
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_child_includes_findings_plan_and_uncommitted_work() {
    let fx = Fx::new(Some(child_runner("Filed 1 findings").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Review it").await;
    fx.te.store.add_finding(Finding {
        id: "f1".into(),
        workspace_id: child.id.clone(),
        file: "x.go".into(),
        line: 10,
        severity: Severity::High,
        message: "nil deref".into(),
        status: FindingStatus::Open.as_str().into(),
        ..Finding::default()
    });
    fx.te.store.update_workspace(&child.id, |w| {
        w.plan = Some(Plan {
            summary: "Ship the fix".into(),
            body: "full plan body".into(),
            steps: vec![PlanStep {
                id: "S1".into(),
                title: "patch x.go".into(),
                ..PlanStep::default()
            }],
            ..Plan::default()
        });
    });
    std::fs::write(
        std::path::Path::new(&child.worktree_path).join("dirty.txt"),
        "dirty\n",
    )
    .unwrap();
    let report = fx.eng().read_child_from_chat(&caller.id, &child.id).await;
    for want in [
        "nil deref",
        "x.go:10",
        "Plan:",
        "full plan body",
        "patch x.go",
        "Uncommitted",
        "dirty.txt",
        "+dirty",
    ] {
        assert!(
            report.contains(want),
            "read_child missing {want}:\n{report}"
        );
    }
    fx.te.close().await;
}

#[test]
fn want_child_defaults() {
    let caller = Workspace {
        project_id: "p".into(),
        branch: "b".into(),
        worktree_path: "/wt".into(),
        ..Workspace::default()
    };
    let scheduled = Workspace {
        schedule_id: "sched".into(),
        ..caller.clone()
    };
    let bare = Workspace {
        project_id: "p".into(),
        ..Workspace::default()
    };
    let nested = Workspace {
        parent_id: "p0".into(),
        ..caller.clone()
    };
    let req = |standalone: Option<bool>, base: &str, project: &str| CreateWorkspaceRequest {
        standalone,
        base_branch: base.into(),
        project_id: project.into(),
        ..CreateWorkspaceRequest::default()
    };
    let cases = [
        (
            "agent chat defaults to child",
            &caller,
            req(None, "", ""),
            CHAT_MODE_AGENT,
            true,
            false,
        ),
        (
            "scheduled caller defaults standalone",
            &scheduled,
            req(None, "", ""),
            CHAT_MODE_AGENT,
            false,
            false,
        ),
        (
            "loop mode defaults standalone",
            &caller,
            req(None, "", ""),
            CHAT_MODE_LOOP,
            false,
            false,
        ),
        (
            "base_branch implies standalone",
            &caller,
            req(None, "main", ""),
            CHAT_MODE_AGENT,
            false,
            false,
        ),
        (
            "explicit standalone",
            &caller,
            req(Some(true), "", ""),
            CHAT_MODE_AGENT,
            false,
            false,
        ),
        (
            "explicit child on scheduled caller",
            &scheduled,
            req(Some(false), "", ""),
            CHAT_MODE_AGENT,
            true,
            false,
        ),
        (
            "other project",
            &caller,
            req(None, "", "q"),
            CHAT_MODE_AGENT,
            false,
            true,
        ),
        (
            "caller without worktree",
            &bare,
            req(None, "", ""),
            CHAT_MODE_AGENT,
            false,
            true,
        ),
        (
            "chat child cannot nest",
            &nested,
            req(None, "", ""),
            CHAT_MODE_AGENT,
            false,
            true,
        ),
        (
            "chat child explicit child refused",
            &nested,
            req(Some(false), "", ""),
            CHAT_MODE_AGENT,
            false,
            true,
        ),
        (
            "chat child standalone refused",
            &nested,
            req(Some(true), "", ""),
            CHAT_MODE_AGENT,
            false,
            true,
        ),
    ];
    for (name, caller, req, mode, child, refuse) in cases {
        let pid = if req.project_id.is_empty() {
            caller.project_id.clone()
        } else {
            req.project_id.clone()
        };
        let got = want_child(caller, &req, mode, &pid);
        assert_eq!(
            (got.as_ref().copied().unwrap_or(false), got.is_err()),
            (child, refuse),
            "{name}: {got:?}"
        );
    }
}

#[tokio::test]
async fn a_chat_child_auto_resumes_its_interrupted_first_turn() {
    let fx = Fx::new(None);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().into_owned();
    let agent = Agent {
        code: PAIR_AGENT_CODE.into(),
        status: "working".into(),
        ..Agent::default()
    };
    let child = Workspace {
        worktree_path: path.clone(),
        scheduled_prompt_pending: true,
        parent_id: "parent".into(),
        fan_out: Some(ChildFanOut {
            chat: "PA".into(),
            ..ChildFanOut::default()
        }),
        ..Workspace::default()
    };
    assert!(fx.eng().should_auto_resume_chat(&child, &agent));
    let scheduled = Workspace {
        worktree_path: path,
        scheduled_prompt_pending: true,
        schedule_id: "s1".into(),
        ..Workspace::default()
    };
    assert!(!fx.eng().should_auto_resume_chat(&scheduled, &agent));
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_is_not_settled_while_ask_user_is_pending() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Need a decision").await;
    assert!(fx.eng().child_settled_for_chat(&child.id));
    fx.eng().ask_user_broker().park(
        UserQuestion {
            workspace_id: child.id.clone(),
            agent_code: PAIR_AGENT_CODE.into(),
            question: "which one?".into(),
            ..UserQuestion::default()
        },
        None,
    );
    assert!(!fx.eng().child_settled_for_chat(&child.id));
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_parent_removes_its_children() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Orphan check").await;
    fx.eng().remove_workspace(&caller.id).await.unwrap();
    drain(&fx.eng().tasks.teardown).await;
    let after = fx.te.store.workspace(&child.id).expect("child row");
    assert!(after.archived, "child left as an orphan: {after:?}");
    assert!(
        !fx.eng()
            .git_manager(&fx.repo.path)
            .base_branch_exists(&child.branch)
            .await
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sibling_children_from_one_turn_share_a_wave() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let eng = fx.eng();
    let made = |title: &'static str| {
        let fx = &fx;
        let caller = &caller;
        async move {
            let r = create(fx, caller, title).await;
            assert!(r.starts_with("Created subagent "), "create {title} = {r}");
            by_goal(fx, title)
        }
    };
    let turn = eng
        .register_chat_turn(&caller.id, "PA2", CancellationToken::new())
        .unwrap();
    let (a, b) = (made("Engine").await, made("CLI").await);
    eng.unregister_chat_turn(&caller.id, "PA2", &turn);
    let wave = |w: &Workspace| w.fan_out.as_ref().map_or(0, |f| f.wave);
    assert!(
        wave(&a) != 0 && wave(&a) == wave(&b),
        "{} {}",
        wave(&a),
        wave(&b)
    );
    let turn2 = eng
        .register_chat_turn(&caller.id, "PA2", CancellationToken::new())
        .unwrap();
    let c = made("Docs").await;
    eng.unregister_chat_turn(&caller.id, "PA2", &turn2);
    assert_eq!(wave(&c), wave(&a) + 1);
    drain(&eng.tasks.schedules).await;
    let cards: Vec<_> = fx
        .te
        .store
        .feed(&caller.id)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.card == FEED_CARD_FAN_OUT)
        .collect();
    assert_eq!(cards.len(), 2, "one card per wave: {cards:?}");
    let wave1 = cards
        .iter()
        .map(|f| serde_json::from_str::<serde_json::Value>(&f.data).unwrap())
        .find(|w| w["wave"].as_i64() == Some(wave(&a)))
        .expect("wave 1 card");
    assert_eq!(
        wave1["children"].as_array().map(Vec::len),
        Some(2),
        "{wave1}"
    );
    assert_eq!(wave1["title"], "Subagents");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_child_awaiting_auto_resume_is_not_settled() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Restart me").await;
    let key = chat_turn_key(&child.id, PAIR_AGENT_CODE);
    fx.eng().resuming_chats.lock().insert(key.clone());
    assert!(!fx.eng().child_settled_for_chat(&child.id));
    fx.eng().resuming_chats.lock().remove(&key);
    assert!(fx.eng().child_settled_for_chat(&child.id));
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_child_on_a_running_child_says_running() {
    let fx = Fx::new(Some(child_runner("Working…").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Slow one").await;
    let turn = fx
        .eng()
        .register_chat_turn(&child.id, PAIR_AGENT_CODE, CancellationToken::new())
        .unwrap();
    let report = fx.eng().read_child_from_chat(&caller.id, &child.id).await;
    fx.eng()
        .unregister_chat_turn(&child.id, PAIR_AGENT_CODE, &turn);
    assert!(
        report.contains("STILL RUNNING")
            && !report.contains("has finished")
            && !report.contains(&format!("merge_child {}", child.id)),
        "{report}"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn a_failed_create_without_a_worktree_still_reports() {
    let fx = Fx::new(None);
    let dir = tempfile::tempdir().unwrap();
    let parent = fx
        .te
        .store
        .add_workspace(Workspace {
            id: "parent".into(),
            goal: "p".into(),
            branch: "b".into(),
            worktree_path: dir.path().to_string_lossy().into_owned(),
            ..Workspace::default()
        })
        .unwrap();
    let child = fx
        .te
        .store
        .add_workspace(Workspace {
            id: "child".into(),
            parent_id: parent.id.clone(),
            goal: "failed spawn".into(),
            last_error: "no terminals".into(),
            phase: "failed".into(),
            fan_out: Some(ChildFanOut {
                chat: "PA".into(),
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    assert!(fx.eng().chat_child_ready_to_report(&child));
    let report = fx.eng().child_report(&parent.id, &child, false).await;
    assert!(report.contains("no terminals"), "{report}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn removing_a_child_stamps_closed() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Spike").await;
    fx.eng().remove_workspace(&child.id).await.unwrap();
    let after = fx.te.store.workspace(&child.id).expect("child row");
    assert!(after.archived);
    assert_eq!(outcome(&fx, &child.id), CHILD_OUTCOME_CLOSED);
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deleting_a_parent_removes_its_children() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Orphan delete").await;
    fx.eng().delete_workspace(&caller.id).await.unwrap();
    drain(&fx.eng().tasks.teardown).await;
    let after = fx.te.store.workspace(&child.id).expect("child row");
    assert!(after.archived, "child left as an orphan: {after:?}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_child_report_keeps_the_parent_tabs_model() {
    let c = child_runner("Done.");
    let fx = Fx::new(Some(c.provider()));
    fx.eng().set_fan_out_poll(Duration::from_millis(50));
    let caller = parent(&fx).await;
    fx.eng().ensure_pair_agent(&caller.id, "PA2", "");
    let f = [("effort".to_owned(), "high".to_owned())]
        .into_iter()
        .collect();
    fx.eng()
        .remember_workspace_chat_selection(
            &caller.id,
            "PA2",
            "cursor/composer-2.5",
            Some(&f),
            "agent",
            None,
        )
        .await;
    let child = spawn_child(&fx, &caller, "Model check").await;
    let report = report_turn(&c, &caller, &child).await;
    assert!(
        report.model.contains("composer"),
        "model = {}",
        report.model
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_discard_risk_includes_children() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Risk check").await;
    let risk = fx.eng().workspace_discard_risk(&caller.id).await;
    assert!(risk.contains("branch will be deleted"), "{risk}");
    let turn = fx
        .eng()
        .register_chat_turn(&child.id, PAIR_AGENT_CODE, CancellationToken::new())
        .unwrap();
    let risk = fx.eng().workspace_discard_risk(&caller.id).await;
    fx.eng()
        .unregister_chat_turn(&child.id, PAIR_AGENT_CODE, &turn);
    assert!(risk.contains("running child"), "{risk}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_report_keeps_an_outcome_set_during_the_parent_turn() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Closed mid-turn").await;
    let set = |o: &str| {
        fx.te.store.update_workspace(&child.id, |w| {
            w.fan_out.get_or_insert_with(ChildFanOut::default).outcome = o.into();
        });
    };
    set(CHILD_OUTCOME_CLOSED);
    fx.eng().mark_child_reported(&caller.id, &child.id, None);
    assert_eq!(outcome(&fx, &child.id), CHILD_OUTCOME_CLOSED);
    set(CHILD_OUTCOME_REPORT_PENDING);
    fx.eng().mark_child_reported(&caller.id, &child.id, None);
    assert_eq!(outcome(&fx, &child.id), CHILD_OUTCOME_SETTLED);
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rearm_does_not_resend_a_delivered_report() {
    let c = child_runner("Done.");
    let fx = Fx::new(Some(c.provider()));
    fx.eng().set_fan_out_poll(Duration::from_millis(50));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Delivered once").await;
    let (st, id) = (Arc::clone(&fx.te.store), child.id.clone());
    wait_for(Duration::from_secs(10), "settled", || {
        st.workspace(&id)
            .and_then(|w| w.fan_out)
            .is_some_and(|f| f.outcome == CHILD_OUTCOME_SETTLED)
    })
    .await;
    let child = fx.ws(&child.id);
    assert!(fx.eng().notice_delivered(&caller.id, "PA2", &child).await);
    fx.te.store.update_workspace(&child.id, |w| {
        w.fan_out.as_mut().unwrap().outcome = CHILD_OUTCOME_REPORT_PENDING.into();
    });
    let runs = c.specs.lock().len();
    fx.eng().rearm_chat_children().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(outcome(&fx, &child.id), CHILD_OUTCOME_SETTLED);
    assert_eq!(
        c.specs.lock().len(),
        runs,
        "rearm resent a delivered report"
    );
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reported_child_reports_again_after_a_new_turn() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    fx.eng().set_fan_out_poll(Duration::from_millis(50));
    let caller = parent(&fx).await;
    let child = spawn_child(&fx, &caller, "Hit the limit").await;
    wait_report(&fx, &caller, &child, 1, "first report delivered").await;
    fx.send(&child.id, PAIR_AGENT_CODE, "Try again.", "").await;
    wait_report(&fx, &caller, &child, 2, "second report delivered").await;
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_child_on_a_finished_child_starts_a_new_turn() {
    let parent_id = Arc::new(parking_lot::Mutex::new(String::new()));
    let parent_runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine: Arc<parking_lot::Mutex<std::sync::Weak<crate::Engine>>> = Arc::default();
    let inner = child_runner("Done.").provider();
    let replies = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    // spawn_child asserts the child settled; the report turn must not
    // message it into a new turn before that.
    let armed = Arc::new(tokio::sync::Notify::new());
    let (pid, runs, weng, rep, arm) = (
        Arc::clone(&parent_id),
        Arc::clone(&parent_runs),
        Arc::clone(&engine),
        Arc::clone(&replies),
        Arc::clone(&armed),
    );
    let provider = before_turn(inner, move |req| {
        let (pid, runs, weng, rep, arm) = (
            Arc::clone(&pid),
            Arc::clone(&runs),
            Arc::clone(&weng),
            Arc::clone(&rep),
            Arc::clone(&arm),
        );
        async move {
            let parent = pid.lock().clone();
            let eng = weng.lock().upgrade();
            if req.workspace_id == parent
                && runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
                && let Some(eng) = eng
            {
                arm.notified().await;
                for w in eng.store.workspaces() {
                    if w.parent_id == parent {
                        let got = eng
                            .message_child_from_chat(&parent, &w.id, "Also add docs.")
                            .await;
                        rep.lock().push(got);
                    }
                }
            }
        }
    });
    let fx = Fx::new(Some(provider));
    *engine.lock() = Arc::downgrade(fx.eng());
    fx.eng().set_fan_out_poll(Duration::from_millis(50));
    let caller = fx.workspace("parent").await;
    *parent_id.lock() = caller.id.clone();
    fx.eng().ensure_pair_agent(&caller.id, PAIR_AGENT_CODE, "");
    let child = spawn_child(&fx, &caller, "Resume me").await;
    armed.notify_one();
    wait_report(
        &fx,
        &caller,
        &child,
        2,
        "report after the report-turn message",
    )
    .await;
    for r in replies.lock().iter() {
        assert!(
            r.contains("new turn"),
            "message_child(report-pending) = {r}"
        );
    }
    assert!(!replies.lock().is_empty());
    let got = fx
        .eng()
        .message_child_from_chat(&caller.id, &child.id, "One more thing.")
        .await;
    assert!(got.contains("new turn"), "message_child(settled) = {got}");
    wait_report(&fx, &caller, &child, 3, "report after the settled message").await;
    fx.eng()
        .set_chat_child_outcome(&caller.id, &child.id, "conflict: README.md", "");
    let got = fx
        .eng()
        .message_child_from_chat(&caller.id, &child.id, "Rebase and fix the conflicts.")
        .await;
    assert!(got.contains("new turn"), "message_child(conflict) = {got}");
    wait_report(&fx, &caller, &child, 4, "report after the conflict message").await;
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_during_a_queued_report_is_honored() {
    let h = HoldInject::new();
    let fx = Fx::new(Some(h.provider()));
    let caller = fx.workspace("parent").await;
    let child = fx
        .te
        .store
        .add_workspace(Workspace {
            id: "proj.kid".into(),
            project_id: fx.proj.clone(),
            parent_id: caller.id.clone(),
            goal: "Kid".into(),
            branch: "kid".into(),
            fan_out: Some(ChildFanOut {
                chat: "PA".into(),
                wave: 1,
                ..ChildFanOut::default()
            }),
            ..Workspace::default()
        })
        .unwrap();
    let (eng, id) = (Arc::clone(fx.eng()), caller.id.clone());
    let first = tokio::spawn(async move {
        eng.send_workspace_message(
            &id,
            "PA",
            "do the thing",
            "claude-code/sonnet",
            None,
            &[],
            "",
            "",
        )
        .await
    });
    within_hang(&h.started, "first turn starts").await;
    let (eng, id, kid) = (Arc::clone(fx.eng()), caller.id.clone(), child.clone());
    let reported = tokio::spawn(async move { eng.report_child(&id, &kid).await });
    // Stop only once the notice has taken its ticket (the live turn holds the first), which
    // fixes the stop generation it answers to: a Stop before that is not "during a queued
    // report", and the notice then rightly starts its own turn.
    let slot = fx.eng().chat_tab_slot(&caller.id, "PA");
    wait_for(Duration::from_secs(5), "report queued", || {
        slot.lock().next >= 2
    })
    .await;
    assert!(fx.eng().cancel_chat(&caller.id, "PA").await);
    h.release.cancel();
    let ok = tokio::time::timeout(Duration::from_secs(5), reported)
        .await
        .expect("report_child returns")
        .unwrap();
    assert!(
        ok,
        "report_child must give up (not retry) after a human Stop"
    );
    join_hang(first, "first send").await;
    assert_eq!(h.runs(), 1, "a stopped notice must not start a turn");
    assert_eq!(outcome(&fx, &child.id), CHILD_OUTCOME_SETTLED);
    let note = fx.ws(&child.id).fan_out.unwrap().note;
    assert!(
        note.contains("cancelled by Stop") && note.contains(&format!("read_child {}", child.id)),
        "{note}"
    );
    let card = fx
        .te
        .store
        .feed(&caller.id)
        .unwrap_or_default()
        .into_iter()
        .rfind(|f| f.card == FEED_CARD_FAN_OUT)
        .expect("fan-out card");
    assert!(card.data.contains("cancelled by Stop"), "{}", card.data);
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_child_refuses_non_children() {
    let fx = Fx::new(Some(child_runner("Done.").provider()));
    let caller = parent(&fx).await;
    let other = fx.workspace("other").await;
    let child = spawn_child(&fx, &caller, "Settled child").await;
    let eng = fx.eng();
    for (p, c) in [
        (caller.id.as_str(), other.id.as_str()),
        ("", other.id.as_str()),
        (other.id.as_str(), child.id.as_str()),
    ] {
        let got = eng.message_child_from_chat(p, c, "steer").await;
        assert!(got.contains("not a child"), "({p}, {c}) = {got}");
    }
    let msg = eng.archive_child_from_chat(&caller.id, &child.id).await;
    assert!(msg.starts_with("Archived"), "{msg}");
    let got = eng
        .message_child_from_chat(&caller.id, &child.id, "steer")
        .await;
    assert!(got.contains("not a child"), "archived = {got}");
    fx.te.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn message_child_injects_into_a_live_turn() {
    let h = HoldInject::new();
    let fx = Fx::new(Some(h.provider()));
    let parent = fx.workspace("msg-parent").await;
    let child = fx.workspace("msg-child").await;
    fx.te
        .store
        .update_workspace(&child.id, |w| w.parent_id = parent.id.clone());
    let (eng, id) = (Arc::clone(fx.eng()), child.id.clone());
    let first = tokio::spawn(async move {
        eng.send_workspace_message(
            &id,
            "PA",
            "do the thing",
            "cursor/composer-2.5",
            None,
            &[],
            "",
            "",
        )
        .await
    });
    within_hang(&h.started, "child turn starts").await;
    let steer = "please stop and fix tests";
    let got = fx
        .eng()
        .message_child_from_chat(&parent.id, &child.id, steer)
        .await;
    assert!(got.contains("mid-turn"), "{got}");
    h.release.cancel();
    let reply = join_hang(first, "child turn").await;
    assert_ne!(reply, CHAT_INJECTED_SENTINEL);
    assert_eq!(
        h.runs(),
        1,
        "message_child must inject, not start a second turn"
    );
    let injected = h.injected.lock().clone();
    assert_eq!(injected.len(), 1, "{injected:?}");
    let (p, display) = &injected[0];
    let display = if display.is_empty() { p } else { display };
    assert_eq!(*display, format!("From parent {}: {steer}", parent.id));
    assert!(
        p.contains(PARENT_STEER_PREFIX.trim()) && p.contains(steer),
        "{p}"
    );
    fx.te.close().await;
}

#[tokio::test]
async fn message_child_steers_a_running_loop() {
    let fx = Fx::new(None);
    let parent = fx
        .te
        .store
        .add_workspace(Workspace {
            id: "msg-loop-parent".into(),
            goal: "parent".into(),
            ..Workspace::default()
        })
        .unwrap();
    let child = fx
        .te
        .store
        .add_workspace(Workspace {
            id: "msg-loop-child".into(),
            parent_id: parent.id.clone(),
            goal: "fan-out".into(),
            parent_step_ids: vec!["S1".into()],
            ..Workspace::default()
        })
        .unwrap();
    let h = walk(&fx.te, &child.id, "", &child.id);
    let steer = "skip the flaky test";
    let got = fx
        .eng()
        .message_child_from_chat(&parent.id, &child.id, steer)
        .await;
    assert!(got.contains("loop"), "{got}");
    assert_eq!(h.lock().pending_steer, steer);
    fx.eng().remove_loop(&h);
    fx.te.close().await;
}
