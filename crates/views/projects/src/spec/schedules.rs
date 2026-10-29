//! The schedule editor, the project's Schedules section and the
//! Schedules page.

use std::sync::Arc;

use gpui::{Entity, TestAppContext};
use gpui_kit::Focusable as _;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Reply, Seed};
use recur_ui::widgets::ToastKind;
use recur_view_core::format::format_medium_short_in;
use recur_view_core::{LocalStorage, ViewName};
use recur_view_home::schedule_editor::{ScheduleEditor, local_time_zone};
use serde_json::{Value, json};

use super::harness::{S, boot, with};
use super::project::open_project;
use crate::project::ProjectView;

type Rows = Arc<Mutex<Vec<Value>>>;

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn editor(s: &mut S) -> Entity<ScheduleEditor> {
    s.cx.update(|_, cx| ScheduleEditor::current(cx))
        .expect("editor")
}

fn focused(s: &mut S, i: usize) -> bool {
    let e = editor(s);
    s.cx.update(|window, cx| {
        let stops = e.read(cx).stops(cx);
        stops.get(i).is_some_and(|h| h.is_focused(window))
    })
}

fn stop_count(s: &mut S) -> usize {
    let e = editor(s);
    s.cx.update(|_, cx| e.read(cx).stops(cx).len())
}

fn project_view(s: &mut S) -> Entity<ProjectView> {
    let shell = s.root.read_with(s.cx, |r, _| r.shell.clone());
    s.cx.update(|_, cx| {
        shell
            .read(cx)
            .route_view(ViewName::Project)
            .and_then(|v| v.downcast::<ProjectView>().ok())
            .expect("project view")
    })
}

fn local_zone_label() -> String {
    local_time_zone().replace('_', " ")
}

#[gpui::test]
fn schedules_save_run_pause_edit_and_delete(cx: &mut TestAppContext) {
    let rows: Rows = Arc::default();
    let workspace = json!({
        "id": "scheduled-test", "projectId": "storefront", "goal": "Check tests",
        "short": "Check tests", "state": "", "phase": "editing", "group": "active",
        "archived": false, "type": "solo", "runStarted": false, "scheduleId": "schedule-test",
        "last": "Chat turn finished", "baseBranch": "main", "branch": "recur/check-tests",
    });
    let (r1, r2, r3, r4, r5) = (
        rows.clone(),
        rows.clone(),
        rows.clone(),
        rows.clone(),
        rows.clone(),
    );
    let ws = workspace.clone();
    let opts = MockOptions::default()
        .resolver("ListScheduledPrompts", move |_, _, _| {
            Reply::value(Value::Array(r1.lock().clone()))
        })
        .resolver("PreviewScheduledPrompt", |a, _, _| {
            if a.first()
                .and_then(|p| p.get("cron"))
                .and_then(Value::as_str)
                == Some("bad cron")
            {
                return Reply::error("Invalid cron expression");
            }
            let n = now();
            Reply::value(json!([n + 86_400_000, n + 172_800_000, n + 259_200_000]))
        })
        .resolver("SaveScheduledPrompt", move |a, _, _| {
            let mut p = a.first().cloned().unwrap_or_default();
            if p.get("id").and_then(Value::as_str).unwrap_or("").is_empty() {
                p["id"] = json!("schedule-test");
            }
            p["nextRunAt"] = json!(now() + 86_400_000);
            let mut list = r2.lock();
            match list.iter().position(|s| s["id"] == p["id"]) {
                Some(i) => list[i] = p.clone(),
                None => list.push(p.clone()),
            }
            Reply::value(p)
        })
        .resolver("RunScheduledPromptNow", move |_, s: &mut Seed, _| {
            let mut list = r3.lock();
            if let Some(first) = list.first_mut() {
                first["lastRun"] = json!({
                    "id": "run-test", "scheduleId": first["id"], "dueAt": now(),
                    "workspaceId": "scheduled-test", "status": "finished",
                    "detail": "Open the workspace to inspect the result.",
                });
            }
            s.insert_loop(ws.clone());
            Reply::value("scheduled-test")
        })
        .resolver("PauseScheduledPrompt", move |a, _, _| {
            let mut list = r4.lock();
            if let Some(p) = list.iter_mut().find(|p| Some(&p["id"]) == a.first()) {
                p["paused"] = a.get(1).cloned().unwrap_or(json!(false));
            }
            Reply::null()
        })
        .resolver("DeleteScheduledPrompt", move |_, _, _| {
            r5.lock().clear();
            Reply::null()
        })
        .resolver("LastAssistantText", |a, _, _| {
            Reply::value(
                if a.first().and_then(Value::as_str) == Some("scheduled-test") {
                    "All tests passed on the nightly run."
                } else {
                    ""
                },
            )
        })
        // Inspect is slower than a double-click; confirm must not wait for it.
        .resolver("WorkspaceDiscardRisk", |_, _, _| Reply::deferred().0);
    let mut s = with(cx, |_| {}, opts);
    open_project(&mut s, "storefront");
    s.click("set-nav:Schedules");
    assert!(s.until(|s| s.has("empty:No schedules yet")));
    s.click("sc-new");
    assert!(s.has("sc-modal:New schedule"));
    assert!(s.has("sel:Repository=storefront"));
    assert!(s.until(|s| s.has("sel:Base branch=main")));
    let e = editor(&mut s);
    let prompt_focus =
        s.cx.update(|_, cx| e.read(cx).prompt().read(cx).focus_handle(cx));
    assert!(
        s.cx.update(|window, _| prompt_focus.is_focused(window)),
        "the prompt has focus"
    );

    // Shift+Tab from the first stop wraps to Simplify (Schedule is disabled); Tab comes back.
    let first = s.cx.update(|_, cx| e.read(cx).stops(cx)[0].clone());
    s.cx.update(|window, cx| window.focus(&first, cx));
    s.keys("shift-tab");
    let last = stop_count(&mut s) - 1;
    assert!(focused(&mut s, last), "Simplify");
    s.keys("tab");
    assert!(focused(&mut s, 0), "Repository");

    s.click("sc-prompt");
    s.type_text("Check tests");
    s.click(&format!("sel:Time zone={}", local_zone_label()));
    s.type_text("New York");
    s.click("sel-opt:America/New York");
    assert!(s.has("sel:Time zone=America/New York"));
    s.click("sel:Repeat=Cron");
    s.click("sel-opt:Once");
    assert!(s.has("inp:Date"));
    s.click("sel:Repeat=Once");
    s.click("sel-opt:Cron");
    assert!(!s.has("inp:Date"));
    s.fill("inp:Cron expression", "bad cron");
    s.wait(300);
    assert!(s.until(|s| s.has("sc-preview:alert=Invalid cron expression")));
    assert!(s.has("sc-go:off"));
    s.fill("inp:Cron expression", "*/15 9-17 * * MON-FRI");
    s.wait(300);
    s.click("sc-help:false");
    assert!(s.has("sc-tip"));
    assert!(s.has("sel:Mode=Ask"));
    s.keys("secondary-.");
    assert!(s.has("sel:Mode=Plan"));
    s.click("sel:Mode=Plan");
    for m in ["Ask", "Plan", "Agent", "Review", "Loop"] {
        assert!(s.has(&format!("sel-opt:{m}")), "{m}");
    }
    s.click("sel-opt:Plan");
    let flags =
        s.cx.update(|_, cx| e.read(cx).draft().map(|d| d.flags.clone()))
            .expect("draft");
    let _ = flags;
    s.click("sel:Model flags=High");
    s.click("sel-opt:Low");
    s.keys("escape");
    assert!(
        s.has("sc-modal:New schedule"),
        "Escape closed the menu, not the dialog"
    );
    s.click("sc-simplify:on");
    assert!(s.has("sc-simplify:off"));
    assert!(s.until(|s| s.has("sc-go:on")));
    s.click("sc-go:on");
    assert!(s.until(|s| s.has("article:Check tests:false")));
    {
        let list = rows.lock();
        let p = &list[0];
        assert_eq!(p["mode"], "plan");
        assert_eq!(p["flags"]["effort"], "low");
        assert_eq!(p["simplify"], false);
        assert_eq!(p["cron"], "*/15 9-17 * * MON-FRI");
        assert_eq!(p["timeZone"], "America/New_York");
    }

    s.hover("article:Check tests:false");
    s.click("sc-btn:Run now:schedule-test");
    assert!(s.until(|s| s.has("sc-status:schedule-test=Finished")));
    s.wire(
        "workspace.updated",
        json!({ "workspaceId": "scheduled-test", "phase": "editing" }),
    );
    assert!(s.until(|s| s.has("sc-runs-n:schedule-test=1")));
    s.click("sc-status:schedule-test=Finished");
    assert!(s.has("sc-runs:Check tests"));
    assert!(s.until(|s| s.has("sc-run-now:scheduled-test=All tests passed on the nightly run.")));
    assert!(!s.has("sc-runs-all:Select all=Select all"));
    s.hover("sc-run:scheduled-test");
    assert!(s.has("sc-run-rm:Remove"));
    s.click("sc-run-rm:Remove");
    s.click("sc-run-rm:Confirm remove");
    assert!(s.until(|s| s.toasted(ToastKind::Success, "Workspace removed")));
    assert!(!s.has("sc-runs:Check tests"));
    assert!(!s.has("sc-runs-n:schedule-test=1"));

    s.hover("article:Check tests:false");
    s.click("sc-btn:Pause:schedule-test");
    assert!(
        s.until(|s| s.has("sc-btn:Resume:schedule-test") || s.has("article:Check tests:false"))
    );
    s.hover("article:Check tests:false");
    assert!(s.has("sc-btn:Resume:schedule-test"));
    s.click("sc-btn:Edit:schedule-test");
    assert!(s.has("sc-modal:Edit schedule"));
    assert!(s.until(|s| s.has("sel:Model flags=Low")));
    assert!(s.has("sc-simplify:off"));
    s.click("sc-prompt");
    s.keys("secondary-a backspace");
    s.type_text("Find missing integration tests");
    s.click("sel:Mode=Plan");
    s.click("sel-opt:Loop");
    assert!(s.until(|s| s.has("sel:Loop=PR loop") || has_prefix(s, "sel:Loop=")));
    s.fill("inp:Cron expression", "0 9 1,15 * *");
    s.wait(300);
    assert!(s.until(|s| s.has("sc-go:on")));
    s.click("sc-go:on");
    assert!(s.until(|_| rows.lock()[0]["prompt"] == "Find missing integration tests"));
    {
        let list = rows.lock();
        let p = &list[0];
        assert_eq!(p["mode"], "loop");
        assert_eq!(p["cron"], "0 9 1,15 * *");
        assert_eq!(p["flags"]["effort"], "low");
        assert_eq!(p["simplify"], false);
        assert!(!p["loopId"].as_str().unwrap_or("").is_empty());
        assert!(!p["policyId"].as_str().unwrap_or("").is_empty());
    }

    s.hover("article:Check tests:false");
    s.click("sc-btn:Delete:schedule-test");
    s.click("sc-btn:Confirm delete:schedule-test");
    assert!(s.until(|s| s.has("empty:No schedules yet")));
    s.click("sc-new");
    s.keys("escape");
    assert!(!s.has("v-schedule-editor"));
    let pv = project_view(&mut s);
    let list =
        s.cx.update(|_, cx| pv.read(cx).schedules.as_ref().map(|(_, l)| l.clone()))
            .expect("list");
    let new_focus = s.cx.update(|_, cx| list.read(cx).new_focus().clone());
    assert!(
        s.cx.update(|window, _| new_focus.is_focused(window)),
        "focus returns to New schedule"
    );
}

fn has_prefix(s: &mut S, prefix: &str) -> bool {
    let loops: Vec<String> = ["PR loop", "Fix loop", "recur.loops.pr", "Pull request"]
        .iter()
        .map(|l| format!("{prefix}{l}"))
        .collect();
    loops.iter().any(|sel| s.has(sel))
}

fn schedule(id: &str, project: &Value, name: &str, extra: Value) -> Value {
    let mut v = json!({
        "id": id, "projectId": project["id"], "name": name, "prompt": "Check tests",
        "baseBranch": project["baseBranch"], "mode": "ask", "model": "claude-code/sonnet",
        "cadence": "daily", "date": "", "time": "09:00", "timeZone": "UTC", "paused": false,
        "nextRunAt": now() + 86_400_000,
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

fn seed_projects() -> Vec<Value> {
    recur_services::mock::fixtures::seed().projects.clone()
}

#[gpui::test]
fn main_navigation_shows_schedules_across_projects_and_restores_after_reload(
    cx: &mut TestAppContext,
) {
    let projects = seed_projects();
    let (first, second) = (projects[0].clone(), projects[1].clone());
    let rows: Rows = Arc::new(Mutex::new(vec![
        schedule(
            "schedule-0",
            &first,
            &format!("Check {}", first["name"].as_str().unwrap()),
            json!({}),
        ),
        schedule(
            "schedule-1",
            &second,
            &format!("Check {}", second["name"].as_str().unwrap()),
            json!({}),
        ),
    ]));
    let (r1, r2) = (rows.clone(), rows.clone());
    let opts = MockOptions::default()
        .resolver("PreviewScheduledPrompt", |_, _, _| {
            Reply::value(json!([now() + 86_400_000]))
        })
        .resolver("ListScheduledPrompts", move |a, _, _| {
            let pid = a.first().and_then(Value::as_str).unwrap_or("").to_string();
            Reply::value(Value::Array(
                r1.lock()
                    .iter()
                    .filter(|p| pid.is_empty() || p["projectId"] == pid.as_str())
                    .cloned()
                    .collect(),
            ))
        })
        .resolver("SaveScheduledPrompt", move |a, _, _| {
            let mut p = a.first().cloned().unwrap_or_default();
            p["id"] = json!("created");
            r2.lock().push(p.clone());
            Reply::value(p)
        });
    let mut s = boot(cx, |_| {}, opts, LocalStorage::in_memory());
    let (n1, n2) = (
        first["name"].as_str().unwrap().to_string(),
        second["name"].as_str().unwrap().to_string(),
    );
    s.click("nav-schedules");
    assert_eq!(s.view(), ViewName::Schedules);
    assert!(s.until(|s| s.has(&format!("article:Check {n1}:false"))));
    assert!(s.has(&format!("article:Check {n2}:false")));
    assert!(s.has(&format!("briefing-h:{n1}")));
    assert!(s.has(&format!("briefing-h:{n2}")));
    s.reload();
    assert_eq!(s.view(), ViewName::Schedules);
    assert!(s.until(|s| s.has("v-schedules")));
    let next = rows
        .lock()
        .iter()
        .map(|p| p["nextRunAt"].as_i64().unwrap_or(0))
        .min()
        .unwrap_or(0);
    let sub = format!(
        "tb-sub=2 active · next {}",
        format_medium_short_in(next, &local_time_zone())
    );
    assert!(s.until(|s| s.has(&sub)), "{sub}");
    s.click("sc-new-bar");
    assert!(s.has("sc-modal:New schedule"));
    s.click(&format!("sel:Repository={n1}"));
    s.click(&format!("sel-opt:{n2}"));
    s.click("sc-prompt");
    s.type_text("New global schedule\nCheck recent changes");
    assert!(s.until(|s| s.has("sc-go:on")));
    s.click("sc-go:on");
    assert!(s.until(|s| s.has("article:New global schedule:false")));
    assert_eq!(rows.lock().last().unwrap()["projectId"], second["id"]);

    open_project(&mut s, first["id"].as_str().unwrap());
    s.click("set-nav:Schedules");
    assert!(s.until(|s| s.has(&format!("article:Check {n1}:false"))));
    assert!(!s.has(&format!("article:Check {n2}:false")));
    // Sidebar hover action, like New workspace on a project row.
    s.hover("nav-schedules");
    s.click("nav-schedules-add");
    assert!(s.has("sc-modal:New schedule"));
    assert_eq!(s.view(), ViewName::Project);
    assert!(!s.has("v-schedules"));
    s.keys("escape");
    // Project row quick link preselects that project.
    s.hover(&format!("ph:{}", second["id"].as_str().unwrap()));
    s.click(&format!("ph-sched:{}", second["id"].as_str().unwrap()));
    assert!(s.has(&format!("sel:Repository={n2}")));
    s.keys("escape");
    assert!(!s.has("v-schedule-editor"));
    // Command palette action.
    s.keys("secondary-k");
    s.type_text("New schedule");
    s.click("pal-row:New schedule:sel");
    assert!(s.has("sc-modal:New schedule"));
}

fn dash_seed(s: &mut Seed, ws: &[Value]) {
    for w in ws {
        s.insert_loop(w.clone());
    }
}

fn run_ws(id: &str, name: &str, schedule: &str, extra: Value) -> Value {
    let mut v = json!({
        "id": id, "projectId": "storefront", "goal": name, "short": name, "state": "",
        "phase": "editing", "group": "active", "archived": false, "type": "solo",
        "runStarted": false, "isMain": false, "scheduleId": schedule, "hostPrId": "",
        "last": "Chat turn finished", "baseBranch": "main", "branch": format!("recur/{id}"),
    });
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        for (k, x) in e {
            o.insert(k.clone(), x.clone());
        }
    }
    v
}

#[gpui::test]
fn schedule_dashboard_remembers_expanded_rows_after_leaving_and_reload(cx: &mut TestAppContext) {
    let project = seed_projects()[0].clone();
    let ws = run_ws(
        "sched-dash-run",
        "Nightly check",
        "schedule-dash",
        json!({}),
    );
    let rows = vec![schedule(
        "schedule-dash",
        &project,
        "Nightly check",
        json!({ "lastRun": { "id": "run-dash", "scheduleId": "schedule-dash", "dueAt": now(), "workspaceId": "sched-dash-run", "status": "finished", "detail": "" } }),
    )];
    let opts = MockOptions::default()
        .resolver("ListScheduledPrompts", move |_, _, _| {
            Reply::value(Value::Array(rows.clone()))
        })
        .resolver("LastAssistantText", |a, _, _| {
            Reply::value(
                if a.first().and_then(Value::as_str) == Some("sched-dash-run") {
                    "All tests passed."
                } else {
                    ""
                },
            )
        });
    let w2 = ws.clone();
    let mut s = boot(
        cx,
        move |sd| dash_seed(sd, &[w2]),
        opts,
        LocalStorage::in_memory(),
    );
    s.click("nav-schedules");
    assert!(s.until(|s| s.has("article:Nightly check:false")));
    s.click("sc-status:schedule-dash=Finished");
    assert!(s.has("article:Nightly check:true"));
    assert!(s.has("sc-runs:Nightly check"));
    s.click("nav-inbox");
    assert!(!s.has("v-schedules"));
    s.click("nav-schedules");
    assert!(s.until(|s| s.has("article:Nightly check:true")));
    assert!(s.has("sc-runs:Nightly check"));
    s.reload();
    assert!(s.until(|s| s.has("article:Nightly check:true")));
    s.click("sc-status:schedule-dash=Finished");
    assert!(s.has("article:Nightly check:false"));
    s.reload();
    assert!(s.until(|s| s.has("article:Nightly check:false")));
}

#[gpui::test]
fn schedule_dashboard_bulk_removes_selected_workspaces(cx: &mut TestAppContext) {
    let project = seed_projects()[0].clone();
    let settled: Vec<Value> = (0..2)
        .map(|i| {
            run_ws(
                &format!("sched-bulk-{i}"),
                &format!("Nightly {}", i + 1),
                "schedule-bulk",
                json!({}),
            )
        })
        .collect();
    let running = run_ws(
        "sched-bulk-live",
        "Nightly live",
        "schedule-bulk",
        json!({ "state": "coding", "type": "loop", "runStarted": true, "last": "Still working" }),
    );
    let rows = vec![schedule(
        "schedule-bulk",
        &project,
        "Nightly bulk",
        json!({ "lastRun": { "id": "run-bulk", "scheduleId": "schedule-bulk", "dueAt": now(), "workspaceId": "sched-bulk-1", "status": "finished", "detail": "" } }),
    )];
    let opts = MockOptions::default()
        .resolver("ListScheduledPrompts", move |_, _, _| {
            Reply::value(Value::Array(rows.clone()))
        })
        .resolver("LastAssistantText", |a, _, _| {
            Reply::value(
                if a.first().and_then(Value::as_str) == Some("sched-bulk-live") {
                    "Still working."
                } else {
                    "All tests passed."
                },
            )
        })
        .resolver("WorkspaceDiscardRisk", |_, _, _| Reply::deferred().0);
    let mut all = settled.clone();
    all.push(running.clone());
    let mut s = boot(
        cx,
        move |sd| dash_seed(sd, &all),
        opts,
        LocalStorage::in_memory(),
    );
    s.click("nav-schedules");
    assert!(s.until(|s| s.has("article:Nightly bulk:false")));
    s.click("sc-status:schedule-bulk=Finished");
    for id in ["sched-bulk-0", "sched-bulk-1", "sched-bulk-live"] {
        assert!(s.has(&format!("sc-run:{id}")), "{id}");
    }
    assert!(s.has("sc-runs-all:Select all=Select all"));
    assert!(!s.has("sc-pick:Select Nightly live:false"));
    s.click("sc-runs-all:Select all=Select all");
    assert!(s.has("sc-runs-all:Deselect all=2 selected"));
    assert!(s.has("sc-pick:Deselect Nightly 1:true"));
    assert!(!s.has("sc-pick:Select Nightly live:false"));
    s.click("sc-bulk-rm:Remove");
    s.click("sc-bulk-rm:Confirm remove");
    assert!(!s.has("sc-run:sched-bulk-0"));
    assert!(!s.has("sc-run:sched-bulk-1"));
    assert!(s.until(|s| s.toasted(ToastKind::Success, "2 workspaces removed")));
    assert!(s.has("sc-run:sched-bulk-live"));
    assert!(!s.has("sc-runs-all:Select all=Select all"));
}

/// `<input type=date|time>` in the editor: fields take arrows, digits and AM/PM letters and
/// Backspace as WebKit's do (en-US: `M/d/y`, `h:mm a`), and the draft follows the composed
/// value, empty while a field is.
#[gpui::test]
fn once_takes_its_date_and_time_from_segmented_fields(cx: &mut TestAppContext) {
    let mut s = with(cx, |_| {}, MockOptions::default());
    open_project(&mut s, "storefront");
    s.click("set-nav:Schedules");
    assert!(s.until(|s| s.has("empty:No schedules yet")));
    s.click("sc-new");
    s.click("sel:Repeat=Cron");
    s.click("sel-opt:Once");
    let e = editor(&mut s);
    let draft = |s: &mut S| {
        s.cx.update(|_, cx| {
            e.read(cx)
                .draft()
                .map(|d| (d.date.clone(), d.time.clone()))
                .expect("draft")
        })
    };
    assert_eq!(draft(&mut s), (String::new(), "09:00".into()));

    s.click("sc-date:field0");
    if cfg!(not(target_os = "macos")) {
        // The click opened WebKitGTK's calendar popover, which takes the keys until Escape.
        assert!(s.has("sc-date-calendar"));
        s.keys("escape");
        assert!(s.has("sc-modal:New schedule"));
    }
    s.type_text("12");
    s.keys("right");
    s.type_text("31");
    s.keys("/");
    s.type_text("2030");
    assert_eq!(draft(&mut s).0, "2030-12-31");
    s.keys("left left up");
    assert_eq!(draft(&mut s).0, "2030-01-31", "December wraps to January");
    s.keys("backspace");
    assert_eq!(draft(&mut s).0, "", "an empty month empties the value");
    s.keys("down");
    assert_eq!(draft(&mut s).0, "2030-12-31");

    s.click("sc-time:field0");
    s.type_text("7");
    s.keys("right");
    s.type_text("45");
    s.keys("right");
    s.type_text("p");
    assert_eq!(draft(&mut s).1, "19:45");
    s.keys("up");
    assert_eq!(draft(&mut s).1, "07:45");
    s.keys("escape");
    assert!(
        !s.has("sc-modal:New schedule"),
        "Escape with no picker open closes the dialog"
    );
}
