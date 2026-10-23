//! `welcome.spec.ts`: the first-run path.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use gpui::TestAppContext;
use recur_services::mock::{MockOptions, Reply, Seed};
use recur_view_core::{LocalStorage, ViewName};
use serde_json::{Value, json};

use super::harness::{S, boot, plain, with};
use crate::new_task::NewTaskModal;

fn codex_login(s: &mut Seed) {
    s.settings["codexChatGPTAuth"] = json!(true);
}

fn first_run() -> MockOptions {
    MockOptions::default()
        .resolver("ListProjects", |_, _, _| Reply::value(json!([])))
        .resolver("ListWorkspaces", |_, _, _| Reply::value(json!([])))
}

fn no_agent(o: MockOptions) -> MockOptions {
    o.resolver("DetectAgent", |_, _, _| Reply::error("no agent"))
}

impl S<'_> {
    fn current_step(&mut self) -> u8 {
        (1..=3)
            .find(|n| self.has(&format!("ws-step:{n}:current")))
            .unwrap_or(4)
    }

    fn get_started(&mut self) {
        self.click("foot-help");
        assert_eq!(self.view(), ViewName::Welcome);
    }

    fn chips(&mut self, step: u8) -> Vec<String> {
        let mut out = Vec::new();
        for i in 0..5 {
            let found = self.labels_with(&format!("ws-chip:{step}:{i}="));
            out.extend(found);
        }
        out
    }

    /// The text after `prefix` of the first selector the probe list finds.
    fn labels_with(&mut self, prefix: &str) -> Vec<String> {
        let candidates = [
            "Claude Code",
            "Cursor",
            "Codex",
            "storefront",
            "payments-api",
            "marketing-site",
            "1 workspace",
        ];
        let mut out = Vec::new();
        for c in candidates {
            if self.has(&format!("{prefix}{c}")) {
                out.push(c.to_string());
            }
        }
        for n in 2..20 {
            let c = format!("{n} workspaces");
            if self.has(&format!("{prefix}{c}")) {
                out.push(c);
            }
        }
        out
    }

    fn h1(&mut self) -> &'static str {
        if self.has("welcome-h1=You’re set up.") {
            "You’re set up."
        } else {
            assert!(self.has("welcome-h1=Set up Recur"));
            "Set up Recur"
        }
    }

    fn new_task_text(&mut self) -> String {
        self.cx.update(|_, cx| {
            NewTaskModal::current(cx)
                .map(|m| m.read(cx).task().to_string())
                .unwrap_or_default()
        })
    }
}

#[gpui::test]
fn a_fresh_app_with_a_detected_agent_lands_on_step_2_and_opens_the_project_dialog(
    cx: &mut TestAppContext,
) {
    let mut s = with(cx, codex_login, first_run());
    assert_eq!(s.view(), ViewName::Welcome);
    assert!(s.has("v-welcome:on"));
    assert!(s.has("wprov:claude:on"));
    assert!(s.has("wprov-note:claude=dev@example.com (Acme)"));
    assert!(s.has("wprov:codex"));
    assert!(s.has("wprov-note:codex=Not signed in. Run codex login in a terminal."));
    assert!(s.has("wprov:cursor:Connect"));
    assert_eq!(s.current_step(), 2);
    s.click("ws-body:Choose a repository");
    assert!(s.has("v-newproject"));
}

#[gpui::test]
fn without_an_agent_step_1_offers_providers_and_can_be_skipped(cx: &mut TestAppContext) {
    let mut s = with(cx, codex_login, no_agent(first_run()));
    assert_eq!(s.current_step(), 1);
    for p in ["claude", "codex", "cursor"] {
        assert!(!s.has(&format!("wprov:{p}:on")), "{p}");
    }
    assert!(s.has("wprov:claude:Check again"));
    assert!(s.has("wprov:codex:Check again"));
    assert!(!s.has("wprov:cursor:Check again"));
    s.click("ws-links:Continue without an agent");
    assert_eq!(s.current_step(), 2);
    for p in ["claude", "codex", "cursor"] {
        assert!(s.has(&format!("wprov:{p}")), "{p} row");
    }
    assert!(!s.has("ws-links:Continue without an agent"));
}

#[gpui::test]
fn a_cursor_key_counts_as_a_connected_agent_without_claude(cx: &mut TestAppContext) {
    let mut s = with(
        cx,
        |s| s.settings["cursorApiKey"] = json!("crsr_test"),
        no_agent(first_run()),
    );
    assert!(s.has("wprov:cursor:on"));
    assert!(!s.has("wprov:claude:on"));
    assert_eq!(s.current_step(), 2);
}

#[gpui::test]
fn a_cursor_key_without_the_sdk_is_not_ready_until_install_sdk_completes(
    cx: &mut TestAppContext,
) {
    let installed = Arc::new(AtomicBool::new(false));
    let status = |on: bool| {
        json!({
            "bridgeInstalled": on, "authenticated": on,
            "email": if on { "dev@example.com" } else { "" },
            "error": if on { "" } else { "cursor-sdk-bridge not found" },
        })
    };
    let (a, b) = (installed.clone(), installed.clone());
    let opts = no_agent(first_run())
        .resolver("GetCursorStatus", move |_, _, _| {
            Reply::value(status(a.load(Ordering::SeqCst)))
        })
        .resolver("InstallCursorBridge", move |_, _, _| {
            b.store(true, Ordering::SeqCst);
            Reply::value(status(true))
        });
    let mut s = with(cx, |s| s.settings["cursorApiKey"] = json!("crsr_test"), opts);
    assert_eq!(s.current_step(), 1);
    assert!(s.has("wprov:cursor:warn"));
    assert!(s.has(
        "wprov-note:cursor=Signed in, but the Cursor SDK is not installed. Recur needs it to run Cursor agents (about 50 MB)."
    ));
    s.click("wprov:cursor:Install SDK");
    assert!(s.has("wprov:cursor:on"));
    assert!(s.has("wprov-note:cursor=dev@example.com"));
    assert_eq!(s.current_step(), 2);
}

#[gpui::test]
fn a_signed_in_codex_counts_and_the_cursor_connect_link_lands_on_providers_settings(
    cx: &mut TestAppContext,
) {
    let opts = no_agent(first_run()).resolver("GetCodexStatus", |_, _, _| {
        Reply::value(json!({
            "binInstalled": true, "authenticated": true, "loginMethod": "ChatGPT",
            "email": "dev@example.com", "error": "",
        }))
    });
    let mut s = with(cx, |_| {}, opts);
    assert!(s.has("wprov:codex:on"));
    assert_eq!(s.current_step(), 2);
    s.click("wprov:cursor:Connect");
    assert_eq!(s.view(), ViewName::Settings);
    assert!(s.has("set-head=Providers"));
}

#[gpui::test]
fn agents_are_probed_only_while_get_started_is_shown_and_again_on_return_from_settings(
    cx: &mut TestAppContext,
) {
    let probes = Arc::new(AtomicUsize::new(0));
    let authed = Arc::new(AtomicBool::new(false));
    let (p, a) = (probes.clone(), authed.clone());
    let opts = MockOptions::default().resolver("GetCodexStatus", move |_, _, _| {
        p.fetch_add(1, Ordering::SeqCst);
        let on = a.load(Ordering::SeqCst);
        Reply::value(json!({
            "binInstalled": true, "authenticated": on, "loginMethod": "ChatGPT",
            "email": if on { "dev@example.com" } else { "" }, "error": "",
        }))
    });
    let mut s = with(cx, |_| {}, opts);
    assert!(!s.has("v-welcome:on"));
    assert_eq!(probes.load(Ordering::SeqCst), 0);
    s.get_started();
    assert!(s.until(|_| probes.load(Ordering::SeqCst) == 1));
    assert_eq!(s.chips(1), vec!["Claude Code"]);
    authed.store(true, Ordering::SeqCst);
    s.click("ws-head-link:Providers");
    assert!(s.has("set-head=Providers"));
    s.get_started();
    assert!(s.until(|s| s.chips(1) == vec!["Claude Code", "Codex"]));
}

#[gpui::test]
fn with_a_project_but_no_workspace_a_mode_row_opens_an_empty_new_workspace_dialog_in_that_mode(
    cx: &mut TestAppContext,
) {
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.newWorkspaceMode", "loop".to_string());
    let opts = MockOptions::default()
        .resolver("ListWorkspaces", |_, _, _| Reply::value(json!([])));
    let mut s = boot(cx, |_| {}, opts, storage);
    s.get_started();
    assert_eq!(s.current_step(), 3);
    for m in ["Ask", "Plan", "Agent", "Review", "Loop"] {
        assert!(s.has(&format!("wmode:{m}")), "{m}");
    }
    s.click("wmode:Review");
    assert!(s.has("v-newtask"));
    assert_eq!(s.new_task_text(), "");
    assert!(s.has("sel:Mode=Review"));
    s.type_text("Review the auth changes.");
    assert_eq!(s.new_task_text(), "Review the auth changes.");
}

#[gpui::test]
fn a_project_whose_only_workspace_is_the_working_copy_still_needs_a_first_task(
    cx: &mut TestAppContext,
) {
    let main: Arc<std::sync::Mutex<Option<Value>>> = Arc::default();
    let m = main.clone();
    let opts = MockOptions::default().resolver("ListWorkspaces", move |_, seed, _| {
        let mut w = m
            .lock()
            .unwrap()
            .get_or_insert_with(|| seed.loops().next().cloned().unwrap_or_default())
            .clone();
        w["id"] = json!("MAIN-storefront");
        w["isMain"] = json!(true);
        w["archived"] = json!(false);
        w["hostPrId"] = json!("");
        Reply::value(json!([w]))
    });
    let mut s = with(cx, |_| {}, opts);
    s.get_started();
    assert_eq!(s.current_step(), 3);
    assert_eq!(s.h1(), "Set up Recur");
}

#[gpui::test]
fn opening_and_cancelling_the_new_workspace_dialog_does_not_complete_the_first_task(
    cx: &mut TestAppContext,
) {
    let opts = MockOptions::default()
        .resolver("ListWorkspaces", |_, _, _| Reply::value(json!([])))
        .resolver("CreateWorkspace", |a, _, _| {
            Reply::value(json!({
                "id": "WS-NEW", "projectId": a[0], "type": "solo", "goal": "New Workspace",
                "short": "New Workspace", "statusLabel": "Creating", "phase": "creating",
                "branch": "recur/new", "baseBranch": "main",
            }))
        });
    let mut s = with(cx, |_| {}, opts);
    s.get_started();
    assert_eq!(s.current_step(), 3);
    s.keys("cmd-n");
    assert!(s.has("v-newtask"));
    assert_eq!(s.current_step(), 3);
    s.keys("escape");
    assert!(!s.has("v-newtask"));
    assert_eq!(s.current_step(), 3);
    assert_eq!(s.h1(), "Set up Recur");
}

#[gpui::test]
fn a_hand_driven_workspace_counts_once_its_first_message_named_it(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("ListWorkspaces", |_, seed, _| {
        let mut w = seed.loops().next().cloned().unwrap_or_default();
        for (k, v) in [
            ("id", json!("WS-1")),
            ("type", json!("solo")),
            ("runStarted", json!(false)),
            ("isMain", json!(false)),
            ("archived", json!(false)),
            ("hostPrId", json!("")),
            ("goal", json!("Add a timeout to the session refresh")),
            ("short", json!("Add a timeout")),
            ("lastActivityAt", json!(0)),
            ("state", json!("")),
        ] {
            w[k] = v;
        }
        Reply::value(json!([w]))
    });
    let mut s = with(cx, |_| {}, opts);
    s.get_started();
    assert_eq!(s.h1(), "You’re set up.");
    assert_eq!(s.chips(3), vec!["1 workspace"]);
}

#[gpui::test]
fn the_plain_workspace_shortcut_still_uses_the_remembered_mode(cx: &mut TestAppContext) {
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.newWorkspaceMode", "plan".to_string());
    let mut s = boot(cx, |_| {}, MockOptions::default(), storage);
    s.keys("cmd-n");
    assert!(s.has("v-newtask"));
    assert_eq!(s.new_task_text(), "");
    assert!(s.has("sel:Mode=Plan"));
}

#[gpui::test]
fn once_a_workspace_exists_the_page_shows_the_completed_steps_and_links_onward(
    cx: &mut TestAppContext,
) {
    let mut s = plain(cx);
    s.get_started();
    assert_eq!(s.h1(), "You’re set up.");
    for n in 1..=3 {
        assert!(s.has(&format!("ws-num:{n}:done")), "step {n} done");
    }
    assert_eq!(s.chips(1), vec!["Claude Code"]);
    assert_eq!(s.chips(2).len(), 3);
    assert!(s.chips(3)[0].ends_with("workspaces"));
    assert!(s.has("ws-head-link:Providers"));
    s.click("wexplore:Inbox");
    assert_eq!(s.view(), ViewName::Briefing);
}

#[gpui::test]
fn a_set_up_user_does_not_see_the_setup_flow_while_agents_are_still_being_probed(
    cx: &mut TestAppContext,
) {
    let release: Arc<std::sync::Mutex<Option<recur_services::mock::Release>>> = Arc::default();
    let r = release.clone();
    let opts = MockOptions::default().resolver("DetectAgent", move |_, _, _| {
        let (reply, rel) = Reply::deferred();
        *r.lock().unwrap() = Some(rel);
        reply
    });
    let mut s = with(cx, |_| {}, opts);
    s.get_started();
    assert_eq!(s.h1(), "You’re set up.");
    assert!(!s.has("ws-step:1:current"));
    assert!(!s.has("ws-links:Continue without an agent"));
    let rel = release.lock().unwrap().take().expect("probe in flight");
    rel.send(Reply::value(json!("Claude Code · dev@example.com (Acme)")));
    assert!(s.until(|s| s.chips(1) == vec!["Claude Code"]));
}

#[gpui::test]
fn fits_a_narrow_pane_in_the_light_theme(cx: &mut TestAppContext) {
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", "paper".to_string());
    let opts = MockOptions::default()
        .resolver("ListWorkspaces", |_, _, _| Reply::value(json!([])));
    let mut s = boot(cx, |_| {}, opts, storage);
    s.cx.simulate_resize(gpui::size(gpui::px(760.), gpui::px(900.)));
    s.settle();
    s.get_started();
    assert_eq!(s.current_step(), 3);
    let page = s.bounds("v-welcome:on").expect("page");
    for sel in ["ws-step:3:current", "wmode:Loop", "ws-num:1:done"] {
        let b = s.bounds(sel).unwrap_or_else(|| panic!("{sel}"));
        assert!(b.right() <= page.right(), "{sel} {b:?} overflows {page:?}");
    }
}
