#![cfg(feature = "mock")]
//! The mock bridge behaviors the e2e specs rely on, driven through the typed
//! services the way a Rust view calls them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use pretty_assertions::assert_eq;
use recur_domain::{Project, ProjectGroup};
use recur_services::api::{EditorBuffer, EditorSessionMeta, EditorSessionPatch, ShellState};
use recur_services::events::{AppEvent, WorkspaceUpdated};
use recur_services::mock::{BridgeError, MockBridge, MockOptions, Reply, fixtures};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

const WORKING: &str = "LOOP-2231";

fn drain(rx: &mut UnboundedReceiver<AppEvent>) -> Vec<AppEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn names(events: &[AppEvent]) -> Vec<&'static str> {
    events.iter().map(AppEvent::name).collect()
}

fn codes(tabs: &[recur_domain::WorkspaceChatTab]) -> Vec<&str> {
    tabs.iter().map(|t| t.code.as_str()).collect()
}

#[tokio::test]
async fn boot_loads_the_seed() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let projects = s.projects.list_projects().await.unwrap();
    assert_eq!(
        projects.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
        ["storefront", "payments", "marketing"]
    );
    assert!(s.projects.list_project_groups().await.unwrap().is_empty());
    let ws = s.workspaces.list_workspaces().await.unwrap();
    assert_eq!(
        ws.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        [
            "LOOP-2224",
            "LOOP-2235",
            "LOOP-2229",
            "LOOP-2231",
            "LOOP-2218"
        ],
        "newest first"
    );
    for w in &ws {
        assert_eq!(w.worktree_path, format!("/mock/worktrees/{}", w.id));
    }
    assert_eq!(s.prs.list_prs().await.unwrap().len(), 3);
    assert_eq!(s.inbox.get_inbox().await.unwrap().len(), 6);
    let res = s.settings.get_resource().await.unwrap();
    assert_eq!(
        (res.loops_running, res.needs_you, res.iters_today),
        (3, 1, 37)
    );
    s.policy.get_policy().await.unwrap();
    s.settings.get_settings().await.unwrap();
    let att = s.workspaces.attention_counts().await.unwrap();
    assert_eq!((att.unseen, att.inbox_unread, att.dock), (0, 4, 4));
    assert!(
        s.terminal
            .list_terminal_dock_chrome()
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.settings.get_instance_label().await.unwrap(), "");
    assert_eq!(
        fixtures::sidebar_workspace_id("Dark mode for the dashboard").as_deref(),
        Some("LOOP-2235")
    );
}

#[tokio::test]
async fn chat_tabs_create_close_reopen_reorder() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let w = &s.workspaces;
    assert_eq!(
        codes(&w.list_workspace_chats(WORKING).await.unwrap()),
        ["MA", "PA"]
    );
    assert_eq!(w.create_workspace_chat(WORKING).await.unwrap().code, "PA2");
    assert_eq!(w.create_workspace_chat(WORKING).await.unwrap().code, "PA3");

    let open = w.close_workspace_chat(WORKING, "PA2").await.unwrap();
    assert_eq!(codes(&open), ["MA", "PA", "PA3"]);
    let closed = w.list_closed_workspace_chats(WORKING).await.unwrap();
    assert_eq!(codes(&closed), ["PA2"]);
    assert_eq!(
        codes(&w.reopen_workspace_chat(WORKING, "PA2").await.unwrap()),
        ["MA", "PA", "PA3", "PA2"]
    );

    let err = w
        .reorder_workspace_chats(WORKING, &["PA".into()])
        .await
        .unwrap_err();
    assert_eq!(err.message, "reorder chats: got 1 codes, have 4 tabs");
    let dup = ["PA", "PA", "MA", "PA2"].map(String::from);
    let err = w.reorder_workspace_chats(WORKING, &dup).await.unwrap_err();
    assert_eq!(err.message, "reorder chats: duplicate tab PA");
    let unknown = ["PA", "XX", "MA", "PA2"].map(String::from);
    let err = w
        .reorder_workspace_chats(WORKING, &unknown)
        .await
        .unwrap_err();
    assert_eq!(err.message, "reorder chats: unknown tab XX");
    let order = ["PA3", "PA", "MA", "PA2"].map(String::from);
    w.reorder_workspace_chats(WORKING, &order).await.unwrap();
    assert_eq!(
        codes(&w.list_workspace_chats(WORKING).await.unwrap()),
        ["PA3", "PA", "MA", "PA2"]
    );

    w.remember_chat_loop_policy(
        WORKING,
        "PA3",
        "recur.loops.docs",
        "recur.policies.cautious",
    )
    .await
    .unwrap();
    let tabs = w.list_workspace_chats(WORKING).await.unwrap();
    assert_eq!(tabs[0].loop_id, "recur.loops.docs");
    assert_eq!(tabs[0].policy_id, "recur.policies.cautious");
}

#[tokio::test]
async fn closing_the_loop_tab_hides_it_and_clears_loop_chat_code() {
    let bridge = MockBridge::fresh(MockOptions::default());
    let s = bridge.services();
    let w = &s.workspaces;
    bridge.with_seed(|seed| {
        let row = seed.find_loop(|l| l["id"] == WORKING).unwrap();
        let by_id = seed.loop_obj(WORKING).unwrap();
        for o in [by_id, row] {
            seed.obj_mut(o)
                .as_object_mut()
                .unwrap()
                .insert("loopChatCode".into(), json!("MA"));
        }
    });
    assert_eq!(
        codes(&w.close_workspace_chat(WORKING, "MA").await.unwrap()),
        ["PA"]
    );
    assert_eq!(w.get_workspace(WORKING).await.unwrap().loop_chat_code, "");
    // The seed's list row is its own object, so only loopById changed.
    let row = w
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == WORKING)
        .unwrap();
    assert_eq!(row.loop_chat_code, "MA");
    assert_eq!(
        codes(&w.reopen_workspace_chat(WORKING, "MA").await.unwrap()),
        ["MA", "PA"]
    );
    assert_eq!(w.get_workspace(WORKING).await.unwrap().loop_chat_code, "MA");
}

#[tokio::test]
async fn one_object_in_both_lists_shares_later_edits() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let w = &s.workspaces;
    w.set_workspace_base_branch(WORKING, "develop")
        .await
        .unwrap();
    w.close_workspace_chat(WORKING, "MA").await.unwrap();
    let row = w
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == WORKING)
        .unwrap();
    assert_eq!(row.base_branch, "develop");
    assert_eq!(
        row.loop_chat_code, "",
        "SetWorkspaceBaseBranch stored one object in both"
    );

    // SetPRDraft does the same for prs / prById, so a later comment shows in ListPRs.
    s.prs.set_pr_draft("482", true).await.unwrap();
    s.prs
        .add_pr_comment("482", "", 0, "hello", 0, "")
        .await
        .unwrap();
    let listed = s
        .prs
        .list_prs()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == "482")
        .unwrap();
    assert!(listed.draft);
    assert_eq!(listed.merge_state_status, "DRAFT");
    assert_eq!(
        listed.comments.as_ref().map(Vec::len).unwrap_or(0),
        s.prs
            .get_pr("482")
            .await
            .unwrap()
            .comments
            .as_ref()
            .map(Vec::len)
            .unwrap_or(0)
    );
    let ready = s.prs.ready_pr("482").await.unwrap();
    assert!(!ready.draft);
    assert_eq!(ready.merge_state_status, "CLEAN");
}

#[tokio::test]
async fn base_branch_rules_and_pr_retarget() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let w = &s.workspaces;
    let err = w
        .set_workspace_base_branch("NOPE", "main")
        .await
        .unwrap_err();
    assert_eq!(err.message, "workspace NOPE not found");
    let err = w
        .set_workspace_base_branch(WORKING, "  ")
        .await
        .unwrap_err();
    assert_eq!(err.message, "base branch is required");
    let own = w.get_workspace(WORKING).await.unwrap().branch;
    let err = w
        .set_workspace_base_branch(WORKING, &own)
        .await
        .unwrap_err();
    assert_eq!(
        err.message,
        "target branch cannot be the workspace's own branch"
    );
    w.set_workspace_base_branch(WORKING, "release")
        .await
        .unwrap();
    let pr = s
        .prs
        .list_prs()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.workspace_id == WORKING)
        .unwrap();
    assert_eq!(pr.base, "release");

    let main = w.open_main_workspace("storefront").await.unwrap();
    assert!(main.is_main);
    let err = w
        .set_workspace_base_branch(&main.id, "main")
        .await
        .unwrap_err();
    assert_eq!(err.message, "the main working copy has no merge base");
}

#[tokio::test]
async fn chat_attention_follows_focus_and_occlusion() {
    let bridge = MockBridge::fresh(MockOptions::default());
    let s = bridge.services();
    let mut rx = s.subscribe();
    s.workspaces
        .set_workspace_chat_focus(WORKING, "MA")
        .await
        .unwrap();
    let a = bridge.attention();
    assert_eq!(
        (a.focus_ws.as_str(), a.focus_code.as_str(), a.occluded),
        (WORKING, "MA", false)
    );

    // A focused, visible tab finishing raises nothing but the idle status.
    bridge.simulate_chat_turn_idle(WORKING, "MA");
    let ev = drain(&mut rx);
    assert_eq!(names(&ev), ["agent.updated"]);
    let AppEvent::AgentUpdated(u) = &ev[0] else {
        unreachable!()
    };
    assert_eq!(
        (u.agent_id.as_str(), u.status.as_str()),
        ("LOOP-2231-MA", "idle")
    );

    // Occluded: unseen, an inbox row, attention + inbox events.
    s.workspaces.set_chat_ui_occluded(true).await.unwrap();
    bridge.simulate_chat_turn_idle(WORKING, "MA");
    let ev = drain(&mut rx);
    assert_eq!(
        names(&ev),
        ["agent.updated", "attention.updated", "inbox.updated"]
    );
    let AppEvent::AttentionUpdated(att) = &ev[1] else {
        unreachable!()
    };
    assert_eq!(
        (att.counts.unseen, att.counts.dock, att.tab_unseen),
        (1, 5, true)
    );
    assert_eq!(att.counts.by_workspace.as_ref().unwrap()[WORKING], 1);
    assert_eq!(att.counts.by_project.as_ref().unwrap()["storefront"], 1);
    let inbox = s.inbox.get_inbox().await.unwrap();
    assert_eq!(inbox[0].id, "ib-chat-LOOP-2231-MA");
    assert_eq!(inbox[0].title, "<b>Loop</b> finished a reply");
    assert_eq!(inbox[0].target, "chat:LOOP-2231:MA");
    let counts = s.workspaces.attention_counts().await.unwrap();
    assert_eq!((counts.unseen, counts.inbox_unread, counts.dock), (1, 5, 5));

    // Un-occluding the focused tab marks it seen after the call returns.
    s.workspaces.set_chat_ui_occluded(false).await.unwrap();
    let ev = drain(&mut rx);
    assert_eq!(names(&ev), ["attention.updated", "inbox.updated"]);
    let AppEvent::AttentionUpdated(att) = &ev[0] else {
        unreachable!()
    };
    assert_eq!(
        (att.counts.unseen, att.tab_unseen, att.code.as_str()),
        (0, false, "MA")
    );
    assert!(bridge.attention().unseen.is_empty());
    assert!(
        s.inbox
            .get_inbox()
            .await
            .unwrap()
            .iter()
            .all(|i| i.kind != "chat")
    );
}

#[tokio::test]
async fn reading_chat_inbox_rows_clears_unseen() {
    let bridge = MockBridge::fresh(MockOptions::default());
    let s = bridge.services();
    let mut rx = s.subscribe();
    bridge.simulate_chat_turn_idle(WORKING, "PA");
    bridge.simulate_chat_turn_idle("LOOP-2229", "PA");
    drain(&mut rx);

    let unread = s
        .inbox
        .mark_inbox_read("ib-chat-LOOP-2231-PA")
        .await
        .unwrap();
    assert_eq!(unread, 5);
    assert_eq!(bridge.attention().unseen.len(), 1);
    assert_eq!(
        names(&drain(&mut rx)),
        ["attention.updated", "inbox.updated"]
    );

    assert_eq!(s.inbox.mark_inbox_read("ib1").await.unwrap(), 4);
    assert!(
        !s.inbox
            .get_inbox()
            .await
            .unwrap()
            .iter()
            .find(|i| i.id == "ib1")
            .unwrap()
            .unread
    );
    assert!(drain(&mut rx).is_empty());

    assert_eq!(s.inbox.mark_all_inbox_read().await.unwrap(), 0);
    let ev = drain(&mut rx);
    let AppEvent::AttentionUpdated(att) = &ev[0] else {
        unreachable!()
    };
    assert!(att.clear_all_unseen);
    let inbox = s.inbox.get_inbox().await.unwrap();
    assert!(inbox.iter().all(|i| !i.unread && i.kind != "chat"));
}

#[tokio::test]
async fn cancelling_the_loop_tab_parks_the_run() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let mut rx = s.subscribe();
    s.workspaces
        .cancel_workspace_message(WORKING, "PA")
        .await
        .unwrap();
    assert!(drain(&mut rx).is_empty(), "only the run's own tab parks it");
    s.workspaces
        .cancel_workspace_message(WORKING, "MA")
        .await
        .unwrap();
    let ev = drain(&mut rx);
    assert_eq!(
        ev,
        [AppEvent::WorkspaceUpdated(Box::new(WorkspaceUpdated {
            workspace_id: WORKING.into(),
            state: Some("paused".into()),
            phase: Some("idle".into()),
            ..Default::default()
        }))]
    );
    let w = s.workspaces.get_workspace(WORKING).await.unwrap();
    assert_eq!((w.state.as_str(), w.phase.as_str()), ("paused", "idle"));
    let row = s
        .workspaces
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == WORKING)
        .unwrap();
    assert_eq!(row.state.as_str(), "paused");
}

#[tokio::test]
async fn editor_session_merges_and_deletes_when_empty() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let e = &s.editor_session;
    assert_eq!(e.get_editor_session(WORKING).await.unwrap(), None);
    let meta = EditorSessionMeta {
        open_files: Some(vec!["src/main.go".into()]),
        selected_path: "src/main.go".into(),
        syntax_by_path: Some(BTreeMap::from([("a.txt".into(), "source.go".into())])),
        left_open: Some(false),
        ..Default::default()
    };
    let mut buffers = BTreeMap::new();
    buffers.insert(
        "src/main.go".into(),
        EditorBuffer {
            draft: "x".into(),
            base_hash: "h".into(),
        },
    );
    e.update_editor_session(
        WORKING,
        &EditorSessionPatch {
            meta: Some(meta.clone()),
            buffers,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let got = e.get_editor_session(WORKING).await.unwrap().unwrap();
    assert_eq!(
        got.open_files.as_deref(),
        Some(&["src/main.go".to_owned()][..])
    );
    assert_eq!(got.buffers["src/main.go"].draft, "x");
    assert_eq!(got.syntax_by_path["a.txt"], "source.go");
    assert_eq!(got.left_open, Some(false));

    // A meta without the syntax maps leaves them alone; an empty one clears.
    let meta2 = EditorSessionMeta {
        syntax_by_path: None,
        ..meta.clone()
    };
    e.update_editor_session(
        WORKING,
        &EditorSessionPatch {
            meta: Some(meta2),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        e.get_editor_session(WORKING)
            .await
            .unwrap()
            .unwrap()
            .syntax_by_path
            .len(),
        1
    );

    let closed = EditorSessionMeta {
        syntax_by_path: Some(BTreeMap::new()),
        ..Default::default()
    };
    let patch = EditorSessionPatch {
        meta: Some(closed),
        drop_buffers: vec!["src/main.go".into()],
        ..Default::default()
    };
    e.update_editor_session(WORKING, &patch).await.unwrap();
    assert_eq!(
        e.get_editor_session(WORKING).await.unwrap(),
        None,
        "empty session is deleted"
    );
}

#[tokio::test]
async fn shell_state_and_schedule_view_persist() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    assert_eq!(
        s.shell.get_shell_state().await.unwrap(),
        ShellState::default()
    );
    let st = ShellState {
        view: "detail".into(),
        workspace_id: WORKING.into(),
        ..Default::default()
    };
    s.shell.set_shell_state(&st).await.unwrap();
    assert_eq!(s.shell.get_shell_state().await.unwrap(), st);
    s.shell
        .set_expanded_schedules(&["sched-1".into()])
        .await
        .unwrap();
    assert_eq!(s.shell.get_expanded_schedules().await.unwrap(), ["sched-1"]);
}

#[tokio::test]
async fn pr_review_drafts_reactions_and_threads() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let p = &s.prs;
    let c = p
        .add_pr_comment("482", "src/a.go", 12, " looks off ", 10, "f-1")
        .await
        .unwrap();
    assert_eq!(c.body_html, "looks off");
    let pr = p.get_pr("482").await.unwrap();
    let pending = &pr.pending_comments[0];
    assert_eq!(
        (pending.file.as_str(), pending.line, pending.side.as_str()),
        ("src/a.go", 12, "RIGHT")
    );
    assert_eq!(
        (pending.start_line, pending.finding_id.as_str()),
        (10, "f-1")
    );
    // Empty body with a finding id drops that draft.
    p.add_pr_comment("482", "src/a.go", 12, "", 0, "f-1")
        .await
        .unwrap();
    assert!(p.get_pr("482").await.unwrap().pending_comments.is_empty());

    p.add_pr_comment("482", "src/b.go", -3, "left side", 0, "")
        .await
        .unwrap();
    let before = p
        .get_pr("482")
        .await
        .unwrap()
        .comments
        .map(|c| c.len())
        .unwrap_or(0);
    let pr = p
        .submit_review("482", "comment", "Overall fine")
        .await
        .unwrap();
    assert!(pr.pending_comments.is_empty());
    let comments = pr.comments.unwrap();
    assert_eq!(comments.len(), before + 2);
    let review = &comments[before];
    let kid = &comments[before + 1];
    assert_eq!(review.body_md, "Overall fine");
    assert_eq!(kid.parent_id, review.id);
    assert_eq!(
        (kid.path.as_str(), kid.line, kid.side.as_str()),
        ("src/b.go", 3, "LEFT")
    );
    assert_eq!(kid.body_html, "<code>src/b.go:3</code>");

    let pr = p
        .toggle_pr_comment_reaction("482", &review.id, "THUMBS_UP")
        .await
        .unwrap();
    let r = &pr.comments.unwrap()[before].reactions;
    assert_eq!(
        (r[0].content.as_str(), r[0].count, r[0].mine),
        ("+1", 1, true)
    );
    let pr = p
        .toggle_pr_comment_reaction("482", &review.id, "+1")
        .await
        .unwrap();
    assert!(pr.comments.unwrap()[before].reactions.is_empty());

    let reply = p.reply_pr_comment("482", &kid.id, "agreed").await.unwrap();
    assert_eq!(reply.in_reply_to_id, kid.id);
    assert_eq!(reply.thread_id, kid.id);
    let pr = p
        .set_pr_thread_resolved("482", &kid.id, true)
        .await
        .unwrap();
    let comments = pr.comments.unwrap();
    assert!(
        comments
            .iter()
            .filter(|c| c.id == kid.id || c.in_reply_to_id == kid.id)
            .all(|c| c.resolved)
    );

    let pr = p
        .update_pr_details("482", "New title", "Body")
        .await
        .unwrap();
    assert_eq!(pr.title, "New title");
    assert!(
        pr.comments
            .unwrap()
            .iter()
            .any(|c| c.kind == "description" && c.body_md == "Body")
    );
    let merged = p.merge_pr("482", "").await.unwrap();
    assert_eq!(
        (merged.state.as_str(), merged.group.as_str()),
        ("Merged", "merged")
    );
}

#[tokio::test]
async fn projects_and_open_path_create_working_copies() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let p = Project {
        name: "shop".into(),
        repo_path: "/code/shop".into(),
        ..Default::default()
    };
    let saved = s.projects.add_project(&p).await.unwrap();
    assert_eq!(
        (
            saved.id.as_str(),
            saved.base_branch.as_str(),
            saved.open,
            saved.is_git
        ),
        ("shop", "origin/main", true, true)
    );
    let main = s
        .workspaces
        .list_workspaces()
        .await
        .unwrap()
        .into_iter()
        .find(|w| w.id == "MAIN-shop")
        .unwrap();
    assert_eq!(
        (main.branch.as_str(), main.worktree_path.as_str()),
        ("main", "/tmp/recur-e2e/MAIN-shop")
    );

    let plain = Project {
        name: "notes".into(),
        repo_path: "/code/no-git/notes".into(),
        ..Default::default()
    };
    assert!(!s.projects.add_project(&plain).await.unwrap().is_git);
    assert!(s.workspaces.get_workspace("MAIN-notes").await.is_err());

    let opened = s
        .workspaces
        .open_path("/Users/me/Downloads/thing")
        .await
        .unwrap();
    assert!(opened.created);
    assert_eq!(opened.project_id, "open-thing");
    assert_eq!(opened.workspace_id, "MAIN-open-thing");
    let again = s
        .workspaces
        .open_path("/Users/me/Downloads/thing")
        .await
        .unwrap();
    assert!(!again.created);
    let main = s
        .workspaces
        .open_main_workspace("storefront")
        .await
        .unwrap();
    assert_eq!(main.id, "MAIN-storefront");
    assert_eq!(
        s.workspaces
            .open_main_workspace("storefront")
            .await
            .unwrap()
            .id,
        main.id
    );

    let g = s
        .projects
        .add_project_group(&ProjectGroup {
            name: "Work".into(),
            open: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(g.id.len(), 36, "uuid assigned");
    s.projects
        .set_project_group("storefront", &g.id)
        .await
        .unwrap();
    s.projects.delete_project_group(&g.id).await.unwrap();
    let sf = s.projects.get_project("storefront").await.unwrap();
    assert_eq!(sf.group_id, "");
}

#[tokio::test]
async fn pr_workspaces_promote_the_chat_host() {
    let bridge = MockBridge::fresh(MockOptions::default());
    // An imported GitHub PR with no Recur workspace yet.
    bridge.with_seed(|seed| {
        seed.pr_by_id_mut("471")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("workspaceId");
    });
    let s = bridge.services();
    let host = s.prs.ensure_pr_chat_host("471").await.unwrap();
    assert_eq!(
        (host.id.as_str(), host.host_pr_id.as_str()),
        ("WS-HOST-471", "471")
    );
    assert_eq!(s.prs.ensure_pr_chat_host("471").await.unwrap().id, host.id);
    let ws = s.prs.create_workspace_from_pr("471").await.unwrap();
    assert_eq!(
        (ws.id.as_str(), ws.host_pr_id.as_str(), ws.r#type.as_str()),
        ("WS-HOST-471", "", "solo")
    );
    assert_eq!(
        s.prs.get_pr("471").await.unwrap().workspace_id,
        "WS-HOST-471"
    );
    let linked = s.prs.create_workspace_from_pr("482").await.unwrap();
    assert_eq!(linked.id, WORKING);
}

#[tokio::test]
async fn overrides_errors_and_deferred_replies() {
    let unresolved = Arc::new(AtomicUsize::new(0));
    let seen = unresolved.clone();
    let (later, release) = Reply::deferred();
    let later = parking_lot::Mutex::new(Some(later));
    let bridge = MockBridge::fresh(
        MockOptions::default()
            .resolver("MergePR", |_a, _s, _t| {
                Reply::Error(BridgeError::new("protected branch"))
            })
            .resolver("ListPRs", move |_a, _s, _t| {
                later.lock().take().unwrap_or_else(Reply::null)
            })
            .resolver("ListTree", |a, _s, _t| {
                Reply::value(json!([{ "name": a[2], "path": a[2] }]))
            })
            .resolver("ListFiles", |_a, _s, _t| {
                Reply::value(json!(["a/b/c.rs", "a/x.rs", "README.md"]))
            })
            .on_unresolved(move |_| {
                seen.fetch_add(1, Ordering::SeqCst);
            }),
    );
    let s = bridge.services();
    assert_eq!(
        s.prs.merge_pr("482", "").await.unwrap_err().message,
        "protected branch"
    );

    let held = tokio::spawn({
        let s = s.clone();
        async move { s.prs.list_prs().await }
    });
    tokio::task::yield_now().await;
    assert!(!held.is_finished());
    release.send(Reply::value(json!([{ "id": "9" }])));
    assert_eq!(held.await.unwrap().unwrap()[0].id, "9");

    let trees = s
        .code
        .list_trees("storefront", WORKING, &["src".into(), "docs".into()])
        .await
        .unwrap();
    assert_eq!(trees["docs"][0].path, "docs");
    let hits = s
        .code
        .search_files("storefront", WORKING, "A", 0, false)
        .await
        .unwrap();
    let hits = hits.hits.unwrap();
    let paths: Vec<_> = hits.iter().map(|h| (h.path.as_str(), h.dir)).collect();
    assert_eq!(
        paths,
        [
            ("a/b/c.rs", false),
            ("a/x.rs", false),
            ("README.md", false),
            ("a/b", true),
            ("a", true)
        ]
    );
    let only = s
        .code
        .search_files("storefront", WORKING, "", 2, true)
        .await
        .unwrap();
    assert_eq!(only.hits.unwrap().len(), 2);

    assert!(
        s.projects
            .list_scheduled_prompts("")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.terminal.attach_terminal("t1").await.unwrap(), "");
    assert_eq!(unresolved.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_calls_hold_their_queued_events() {
    let bridge = MockBridge::fresh(
        MockOptions::default()
            .resolver("CancelWorkspaceMessage", |_a, _s, _t| Reply::error("nope")),
    );
    let s = bridge.services();
    let mut rx = s.subscribe();
    assert!(
        s.workspaces
            .cancel_workspace_message(WORKING, "MA")
            .await
            .is_err()
    );
    assert!(drain(&mut rx).is_empty());
    s.workspaces.watch_workspace(WORKING).await.unwrap();
    assert_eq!(names(&drain(&mut rx)), ["workspace.updated"]);
}

#[tokio::test]
async fn bundles_catalog_from_shipped_manifests() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let b = &s.bundle;
    let grammars = b.list_grammars().await.unwrap();
    assert_eq!(grammars.len(), 1);
    assert_eq!(
        (
            grammars[0].scope_name.as_str(),
            grammars[0].line_comment.as_str()
        ),
        ("source.go", "//")
    );
    assert!(
        b.read_grammar("source.go")
            .await
            .unwrap()
            .contains("\"scopeName\"")
    );
    let themes = b.list_themes().await.unwrap();
    assert_eq!(
        themes.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["recur", "paper", "cool-glow"]
    );
    assert_eq!(themes[0].preview.r#fn, "#c4a0f5");
    assert!(b.read_theme("paper").await.is_ok());
    assert_eq!(
        b.read_theme("nope").await.unwrap_err().message,
        "theme nope not found"
    );
    let term = b.list_terminal_themes().await.unwrap();
    assert!(
        term.iter()
            .any(|t| t.id == "vga" && !t.palette.background.is_empty())
    );

    // The TypeScript names ListLoops twice; the UI got the workspace list.
    let loops = b.list_loops().await.unwrap();
    assert_eq!(loops.len(), 5);
    assert_eq!(loops[0].id, "LOOP-2218");

    let cmd = b
        .run_command(
            "recur.go.gofmt-on-save",
            &recur_services::api::RunContextDto {
                input: "a  \n\n\n\nb".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(cmd.stdout, "a\n\nb\n");
    let ctx = recur_services::api::PromptContextDto {
        surface: "github_pr".into(),
        ..Default::default()
    };
    let r = b
        .run_prompt_action("recur.chat.review", &ctx)
        .await
        .unwrap();
    assert_eq!(r.display, "Please review this GitHub pull request");
    let req = recur_services::api::PromptCompleteRequestDto {
        args: "li".into(),
        ..Default::default()
    };
    let hits = b
        .run_prompt_complete("recur.chat.fix-checks", &req)
        .await
        .unwrap()
        .hits
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.insert.as_str()).collect::<Vec<_>>(),
        ["lint"]
    );
    assert!(
        b.compiled_module("recur.text.sort-lines-asc")
            .await
            .unwrap()
            .contains("localeCompare")
    );
}

#[tokio::test]
async fn briefing_buckets_the_seed() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let b = s.briefing.get_briefing().await.unwrap();
    assert_eq!(
        (b.counts.needs, b.counts.working, b.counts.pickup),
        (1, 3, 1)
    );
    let workspaces = b.workspaces.unwrap();
    let needs = workspaces.iter().find(|w| w.bucket == "needs").unwrap();
    assert_eq!(needs.workspace_id, "LOOP-2218");
    assert_eq!(needs.primary.target, "workspace:LOOP-2218");
    let pickup = workspaces.iter().find(|w| w.bucket == "pickup").unwrap();
    assert_eq!(
        pickup.secondary[0].target,
        format!("chat:{}:PA", pickup.workspace_id)
    );
    let tabs = s.briefing.list_app_chats().await.unwrap();
    assert_eq!(codes(&tabs), ["APP"]);
    assert_eq!(s.briefing.new_app_chat().await.unwrap().code, "AP2");
    assert_eq!(
        codes(&s.briefing.close_app_chat("AP2").await.unwrap()),
        ["APP"]
    );
}

#[tokio::test]
async fn workspace_creation_shapes() {
    let s = MockBridge::fresh(MockOptions::default()).services();
    let w = &s.workspaces;
    let lp = w
        .create_workspace("storefront", "", "", "loop", "do it", "", None)
        .await
        .unwrap();
    assert_eq!(
        (lp.id.as_str(), lp.loop_id.as_str(), lp.policy_id.as_str()),
        ("LOOP-NEW", "recur.loops.pr", "recur.policies.balanced")
    );
    assert_eq!(lp.base_branch, "main");
    let solo = w
        .create_workspace_with(
            "storefront",
            "",
            "develop",
            "agent",
            "",
            "",
            "recur.loops.docs",
            "",
            None,
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        (
            solo.id.as_str(),
            solo.base_branch.as_str(),
            solo.loop_id.as_str()
        ),
        ("WS-NEW", "develop", "recur.loops.docs")
    );
    let err = w
        .set_workspace_loop_policy("NOPE", "a", "b")
        .await
        .unwrap_err();
    assert_eq!(err.message, "workspace NOPE not found");
    let upd = w
        .set_workspace_loop_policy(WORKING, "recur.loops.docs", "")
        .await
        .unwrap();
    assert_eq!(upd.loop_id, "recur.loops.docs");
    // The mock returns the workspace object for this string method; the UI
    // only tested it for truthiness.
    assert_eq!(
        w.open_workspace_pr(WORKING, false).await.unwrap(),
        "[object Object]"
    );
    let removed = w.remove_workspace("LOOP-2235").await.unwrap();
    assert!(removed.archived);
    assert_eq!(removed.worktree_path, "");
}

#[tokio::test]
async fn dispatched_wire_events_decode() {
    let bridge = MockBridge::fresh(MockOptions::default());
    let mut rx = bridge.services().subscribe();
    bridge
        .dispatch_wire(
            "workspace.updated",
            json!({ "workspaceId": WORKING, "phase": "working", "iteration": 1 }),
        )
        .unwrap();
    bridge
        .dispatch_wire("session.delta", json!({ "workspaceId": WORKING, "code": "PA", "text": "hi", "seq": 2, "replace": true }))
        .unwrap();
    bridge
        .dispatch_wire("term.data", json!({ "id": "t1", "data": "aGk=" }))
        .unwrap();
    bridge
        .dispatch_wire(
            "workspace.event",
            json!({ "workspaceId": WORKING, "event": { "text": "edited a file" } }),
        )
        .unwrap();
    let ev = drain(&mut rx);
    let AppEvent::WorkspaceUpdated(u) = &ev[0] else {
        unreachable!()
    };
    assert_eq!(
        (u.phase.as_deref(), u.iteration, u.state.is_none()),
        (Some("working"), Some(1), true)
    );
    let AppEvent::SessionDelta(d) = &ev[1] else {
        unreachable!()
    };
    assert_eq!(
        (d.tag.code.as_str(), d.text.as_str(), d.seq, d.replace),
        ("PA", "hi", Some(2), true)
    );
    assert_eq!(
        ev[2],
        AppEvent::Term(recur_term::Event::Data {
            id: "t1".into(),
            data: b"hi".to_vec()
        })
    );
    let AppEvent::WorkspaceEvent(e) = &ev[3] else {
        unreachable!()
    };
    assert_eq!(e.event.text, "edited a file");
    assert!(bridge.dispatch_wire("loop.event", Value::Null).is_err());
}
