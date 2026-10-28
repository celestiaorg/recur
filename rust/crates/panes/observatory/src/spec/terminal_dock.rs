//! `terminal-dock.spec.ts`: the dock's contract with the terminal service (adopt, restore,
//! stream and ack, park on hide, close), its tabs and keys, maximize / beside / hide editor,
//! per-workspace chrome, keep-alive across workspace switches, and the run controls. The
//! ⌃` / ⌃⇧` bindings and the ⌘R / ⌘⇧R palettes belong to the shell and the palettes; their
//! effects on the dock (the dock chrome flags, run requests) are driven here through the
//! app state they set. Xterm's DOM class collisions have no GPUI counterpart.

use std::sync::Arc;

use base64::Engine as _;
use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Reply};
use serde_json::{Value, json};

use super::harness::*;
use crate::model::FocusZone;

type CallLog = Arc<Mutex<Vec<(String, Vec<Value>)>>>;

/// Every terminal call, answered the way Go would (`terminalResolvers`).
#[derive(Clone, Default)]
struct Term {
    calls: CallLog,
    roster: Arc<Mutex<Vec<Value>>>,
    restored: Arc<Mutex<Vec<Value>>>,
    chrome: Arc<Mutex<serde_json::Map<String, Value>>>,
    n: Arc<Mutex<u32>>,
}

fn session(id: &str, ws: &str) -> Value {
    json!({
        "id": id, "workspaceId": ws, "cwd": "/tmp/wt", "shell": "/bin/zsh",
        "title": "zsh", "createdAt": "2026-01-01T00:00:00Z",
    })
}

impl Term {
    fn calls(&self, name: &str) -> Vec<Vec<Value>> {
        self.calls
            .lock()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, a)| a.clone())
            .collect()
    }

    fn opts(&self, opts: MockOptions) -> MockOptions {
        let rec = |t: &Term, name: &'static str| {
            let calls = t.calls.clone();
            move |a: &[Value],
                  _: &mut recur_services::mock::Seed,
                  _: &recur_services::mock::Table| {
                calls.lock().push((name.into(), a.to_vec()));
                Reply::null()
            }
        };
        let (t1, t2, t3, t4, t5, t6, t7, t8, t9) = (
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
            self.clone(),
        );
        opts.resolver("ListTerminals", move |a, _, _| {
            t1.calls.lock().push(("ListTerminals".into(), a.to_vec()));
            let ws = a.first().and_then(Value::as_str).unwrap_or("");
            Reply::value(json!(
                t1.roster
                    .lock()
                    .iter()
                    .filter(|s| s["workspaceId"] == ws)
                    .cloned()
                    .collect::<Vec<_>>()
            ))
        })
        .resolver("RestoreWorkspaceTerminals", move |a, _, _| {
            t2.calls
                .lock()
                .push(("RestoreWorkspaceTerminals".into(), a.to_vec()));
            let ws = a.first().and_then(Value::as_str).unwrap_or("").to_string();
            let mut roster = t2.roster.lock();
            if roster.iter().any(|s| s["workspaceId"] == ws.as_str()) {
                return Reply::value(json!(
                    roster
                        .iter()
                        .filter(|s| s["workspaceId"] == ws.as_str())
                        .cloned()
                        .collect::<Vec<_>>()
                ));
            }
            let restored = t2.restored.lock().clone();
            roster.extend(restored.clone());
            Reply::value(json!(restored))
        })
        .resolver("ListTerminalDockChrome", move |_, _, _| {
            Reply::value(json!(
                t3.chrome.lock().values().cloned().collect::<Vec<_>>()
            ))
        })
        .resolver("SetTerminalDockChrome", move |a, _, _| {
            t4.calls
                .lock()
                .push(("SetTerminalDockChrome".into(), a.to_vec()));
            let ws = a.first().and_then(Value::as_str).unwrap_or("").to_string();
            let c = a.get(1).cloned().unwrap_or_default();
            let mut chrome = t4.chrome.lock();
            let flag = |k: &str| c.get(k).and_then(Value::as_bool).unwrap_or(false);
            let pct = c.get("besidePct").and_then(Value::as_f64).unwrap_or(0.) as i64;
            if !flag("open") && !flag("beside") && pct <= 0 {
                chrome.remove(&ws);
            } else {
                chrome.insert(
                    ws.clone(),
                    json!({
                        "workspaceId": ws, "open": flag("open"), "maximized": flag("maximized"),
                        "editorHidden": flag("editorHidden"), "beside": flag("beside"),
                        "besidePct": pct,
                    }),
                );
            }
            Reply::null()
        })
        .resolver("OpenTerminal", move |a, _, _| {
            t5.calls.lock().push(("OpenTerminal".into(), a.to_vec()));
            let mut n = t5.n.lock();
            *n += 1;
            let ws = a.first().and_then(Value::as_str).unwrap_or("");
            let s = session(&format!("sess-{n}"), ws);
            t5.roster.lock().push(s.clone());
            Reply::value(s)
        })
        .resolver("AttachTerminal", move |a, _, _| {
            t6.calls.lock().push(("AttachTerminal".into(), a.to_vec()));
            Reply::value(
                base64::engine::general_purpose::STANDARD.encode("replayed-scrollback\r\n"),
            )
        })
        .resolver("WriteTerminal", move |a, _, _| {
            t7.calls.lock().push(("WriteTerminal".into(), a.to_vec()));
            Reply::null()
        })
        .resolver("CloseTerminal", move |a, _, _| {
            t8.calls.lock().push(("CloseTerminal".into(), a.to_vec()));
            let id = a.first().and_then(Value::as_str).unwrap_or("").to_string();
            t8.roster.lock().retain(|s| s["id"] != id.as_str());
            Reply::null()
        })
        .resolver("ReorderTerminals", move |a, _, _| {
            t9.calls
                .lock()
                .push(("ReorderTerminals".into(), a.to_vec()));
            let ws = a.first().and_then(Value::as_str).unwrap_or("").to_string();
            let ids: Vec<String> =
                serde_json::from_value(a.get(1).cloned().unwrap_or_default()).unwrap_or_default();
            let mut roster = t9.roster.lock();
            let (mine, others): (Vec<Value>, Vec<Value>) = roster
                .drain(..)
                .partition(|s| s["workspaceId"] == ws.as_str());
            if ids.len() != mine.len() {
                return Reply::error("ReorderTerminals: not a permutation");
            }
            *roster = others;
            for id in ids {
                if let Some(s) = mine.iter().find(|s| s["id"] == id.as_str()) {
                    roster.push(s.clone());
                }
            }
            Reply::null()
        })
        .resolver("DetachTerminal", rec(self, "DetachTerminal"))
        .resolver("AckTerminal", rec(self, "AckTerminal"))
        .resolver("ResizeTerminal", rec(self, "ResizeTerminal"))
        .resolver("ClearTerminal", rec(self, "ClearTerminal"))
        .resolver("InterruptTerminal", rec(self, "InterruptTerminal"))
        .resolver("ListLoopAgents", |_, _, _| Reply::value(json!([])))
    }
}

/// The Run commands the project's `project.toml` declares (`seedWithRuns`).
fn with_runs(runs: &[(&str, &str)]) -> impl FnOnce(&mut recur_services::mock::Seed) {
    let runs: Vec<Value> = runs
        .iter()
        .map(|(n, c)| json!({"name": n, "cmd": c}))
        .collect();
    move |s| {
        if let Some(p) = s.projects.iter_mut().find(|p| p["id"] == "storefront") {
            p["runCommands"] = json!(runs);
        }
    }
}

/// The dock's own chords: ⌘ on macOS; on Linux Ctrl+Shift, since the terminal keeps Ctrl chords
/// (`dock_key`, as `TerminalDock.svelte`).
fn dock_chord(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("cmd-{key}")
    } else {
        format!("ctrl-shift-{key}")
    }
}

fn dock_tabs(h: &mut H) -> Vec<(String, String, bool)> {
    h.read(|o, cx| {
        o.dock
            .as_ref()
            .map(|(_, d)| {
                d.read(cx)
                    .tabs
                    .iter()
                    .map(|t| (t.id.clone(), t.title.clone(), t.exited))
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn active_term(h: &mut H) -> String {
    h.read(|o, cx| {
        o.dock
            .as_ref()
            .map(|(_, d)| d.read(cx).active_id.clone())
            .unwrap_or_default()
    })
}

fn dock_open(h: &mut H) -> bool {
    h.has("term-dock")
}

fn term_lit(h: &mut H) -> bool {
    h.read(|o, _| o.focus_zone == FocusZone::Terminal)
}

fn term_out(h: &mut H, id: &str, text: &str) {
    h.event(
        "term.data",
        json!({"id": id, "data": base64::engine::general_purpose::STANDARD.encode(text)}),
    );
}

fn open_dock(h: &mut H) {
    h.click("obs-term-toggle");
    h.wait(100);
    assert!(dock_open(h));
}

#[gpui::test]
async fn toggle_opens_a_shell_streams_output_and_acks_it(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    assert!(h.has("term-pane:sess-1"));
    let opens = t.calls("OpenTerminal");
    assert_eq!(opens.len(), 1);
    assert!(!t.calls("AttachTerminal").is_empty());
    // Opened at the dock's measured geometry, not a blind 80×24.
    let (cols, rows) = (
        opens[0][1].as_i64().unwrap_or(0),
        opens[0][2].as_i64().unwrap_or(0),
    );
    assert!(cols >= 20 && rows > 3, "{cols}×{rows}");
    term_out(&mut h, "sess-1", &"x".repeat(40 * 1024));
    h.wait(200);
    let acks = t.calls("AckTerminal");
    assert!(!acks.is_empty());
    assert_eq!(acks[0][0], "sess-1");
    assert!(acks[0][1].as_i64().unwrap_or(0) >= 32 * 1024);
}

#[gpui::test]
async fn dropping_an_explorer_file_or_folder_onto_the_terminal_inserts_the_path(
    cx: &mut TestAppContext,
) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    let root = format!("/mock/worktrees/{WS}");
    for (sel, path) in [
        ("obs-node:file:src/main.go", "src/main.go"),
        ("obs-node:dir:src", "src"),
    ] {
        h.drag(sel, "term-pane:sess-1");
        let abs = format!("{root}/{path}");
        let hits = t
            .calls("WriteTerminal")
            .iter()
            .filter(|a| {
                a.get(1)
                    .and_then(Value::as_str)
                    .is_some_and(|s| s.trim() == abs)
            })
            .count();
        assert_eq!(hits, 1, "{path}");
    }
}

#[gpui::test]
async fn reopening_adopts_the_running_shell_instead_of_spawning_another(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut live = session("sess-live", WS);
    live["title"] = json!("npm run dev");
    t.roster.lock().push(live);
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    assert_eq!(dock_tabs(&mut h)[0].1, "npm run dev");
    assert!(t.calls("OpenTerminal").is_empty());
    // Hiding parks the view (Go stays attached); only Close kills the shell.
    h.click("term-act:hide");
    assert!(!dock_open(&mut h));
    assert!(t.calls("CloseTerminal").is_empty());
    assert!(t.calls("DetachTerminal").is_empty());
}

#[gpui::test]
async fn restart_restore_respawns_saved_tabs_instead_of_a_blank_shell(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut a = session("sess-a", WS);
    a["title"] = json!("server");
    let mut b = session("sess-b", WS);
    b["cwd"] = json!("/tmp/other");
    b["createdAt"] = json!("2026-01-01T00:00:01Z");
    b["active"] = json!(true);
    *t.restored.lock() = vec![a, b];
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    let tabs = dock_tabs(&mut h);
    assert_eq!(tabs.len(), 2);
    assert_eq!(tabs[0].1, "server");
    assert_eq!(active_term(&mut h), "sess-b");
    assert!(t.calls("OpenTerminal").is_empty());
    assert_eq!(t.calls("RestoreWorkspaceTerminals").len(), 1);
}

#[gpui::test]
async fn tabs_open_switch_and_close(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    assert_eq!(dock_tabs(&mut h).len(), 1);
    h.click("term-act:new");
    h.wait(100);
    assert_eq!(dock_tabs(&mut h).len(), 2);
    assert_eq!(active_term(&mut h), "sess-2");
    // Both panes stay mounted so a background shell keeps a live screen.
    let panes = h.read(|o, cx| o.dock.as_ref().map_or(0, |(_, d)| d.read(cx).panes.len()));
    assert_eq!(panes, 2);
    h.click("tab-chip:term-tab-sess-1");
    assert_eq!(active_term(&mut h), "sess-1");
    h.click("tab-close:term-tab-sess-1");
    h.wait(100);
    assert_eq!(dock_tabs(&mut h).len(), 1);
    assert!(!t.calls("CloseTerminal").is_empty());
    // Closing the last tab closes the drawer.
    h.click("tab-close:term-tab-sess-2");
    h.wait(100);
    assert!(!dock_open(&mut h));
}

#[gpui::test]
async fn dragging_a_terminal_tab_reorders_the_strip_and_survives_remount(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    h.click("term-act:new");
    h.wait(100);
    assert_eq!(dock_tabs(&mut h)[0].0, "sess-1");
    let first = h.bounds("tab-chip:term-tab-sess-1").expect("first");
    let second = h.bounds("tab-chip:term-tab-sess-2:on").expect("second");
    h.drag_points(
        second.center(),
        gpui::point(first.left() + gpui::px(8.), first.center().y),
    );
    h.wait(100);
    assert_eq!(dock_tabs(&mut h)[0].0, "sess-2");
    let last = t
        .calls("ReorderTerminals")
        .last()
        .cloned()
        .expect("reorder");
    assert_eq!(last[1][0], "sess-2");
    h.click("obs-term-toggle");
    assert!(!dock_open(&mut h));
    open_dock(&mut h);
    let ids: Vec<String> = dock_tabs(&mut h).into_iter().map(|t| t.0).collect();
    assert_eq!(ids, vec!["sess-2", "sess-1"]);
}

#[gpui::test]
async fn cmd_t_and_cmd_w_open_and_close_tabs_while_the_dock_is_focused(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    h.click("term-pane:sess-1");
    assert!(term_lit(&mut h));
    h.keys(&dock_chord("t"));
    h.wait(100);
    assert_eq!(dock_tabs(&mut h).len(), 2);
    assert_eq!(active_term(&mut h), "sess-2");
    h.keys(&dock_chord("w"));
    h.wait(100);
    assert_eq!(dock_tabs(&mut h).len(), 1);
    assert_eq!(t.calls("CloseTerminal").len(), 1);
    // Focus elsewhere: the chords leave the dock alone.
    h.focus_editor();
    h.update(|o, window, cx| window.focus(&o.main_focus, cx));
    h.keys(&dock_chord("t"));
    h.wait(100);
    assert_eq!(dock_tabs(&mut h).len(), 1);
}

#[gpui::test]
async fn cmd_f_opens_the_terminal_find_bar_while_the_dock_is_focused(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    h.click("term-pane:sess-1");
    h.keys(&dock_chord("f"));
    let open = h.read(|o, cx| o.dock.as_ref().is_some_and(|(_, d)| d.read(cx).find_open));
    assert!(open);
}

#[gpui::test]
async fn opt_cmd_arrows_rotate_through_tabs_while_the_dock_is_focused(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    h.click("term-act:new");
    h.click("term-act:new");
    h.wait(100);
    assert_eq!(dock_tabs(&mut h).len(), 3);
    assert_eq!(active_term(&mut h), "sess-3");
    h.click("term-pane:sess-3");
    h.keys("secondary-alt-left");
    assert_eq!(active_term(&mut h), "sess-2");
    h.keys("secondary-alt-left");
    assert_eq!(active_term(&mut h), "sess-1");
    h.keys("secondary-alt-left");
    assert_eq!(active_term(&mut h), "sess-3");
    h.keys("secondary-alt-right");
    assert_eq!(active_term(&mut h), "sess-1");
    h.update(|o, window, cx| window.focus(&o.main_focus, cx));
    h.keys("secondary-alt-right");
    assert_eq!(active_term(&mut h), "sess-1");
}

#[gpui::test]
async fn the_active_tab_underline_tracks_which_panel_has_focus(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.keys("secondary-p");
    h.type_text("main.go");
    h.wait(100);
    assert!(h.has("qo-row:0"));
    h.keys("enter");
    h.wait(100);
    assert!(h.active_tab("src/main.go"));
    assert_eq!(h.read(|o, _| o.focus_zone), FocusZone::Editor);
    open_dock(&mut h);
    h.click("term-pane:sess-1");
    assert!(term_lit(&mut h));
    h.focus_editor();
    assert_eq!(h.read(|o, _| o.focus_zone), FocusZone::Editor);
    // The explorer is part of the editor workbench.
    h.update(|o, window, cx| window.focus(&o.files_focus, cx));
    assert_eq!(h.read(|o, _| o.focus_zone), FocusZone::Editor);
}

#[gpui::test]
async fn cmd_1_2_3_focus_agent_editor_and_terminal(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.keys("secondary-3");
    h.wait(400);
    assert!(dock_open(&mut h));
    let focused = h.cx.update(|window, cx| {
        h.obs
            .read(cx)
            .dock
            .as_ref()
            .is_some_and(|(_, d)| d.read(cx).contains_focus(window, cx))
    });
    assert!(focused);
    assert!(term_lit(&mut h));
    h.keys("secondary-2");
    assert!(!term_lit(&mut h));
    h.events.borrow_mut().clear();
    h.keys("secondary-1");
    h.wait(100);
    assert!(h.has("obs-left"));
    assert!(
        h.events
            .borrow()
            .contains(&crate::ObservatoryEvent::FocusAgentComposer)
    );
    assert!(!term_lit(&mut h));
    // From another left panel, ⌘1 switches back to the agent.
    h.click("panel-toggle:findings");
    h.events.borrow_mut().clear();
    h.keys("secondary-1");
    h.wait(100);
    assert_eq!(
        h.read(|o, _| o.left_kind),
        recur_view_core::prefs::CodePanelKey::Maestro
    );
    assert!(
        h.events
            .borrow()
            .contains(&crate::ObservatoryEvent::FocusAgentComposer)
    );
}

#[gpui::test]
async fn an_exited_shell_keeps_its_tab_so_the_final_output_stays_readable(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    h.event(
        "term.exit",
        json!({"id": "sess-1", "workspaceId": WS, "exitCode": 127}),
    );
    let tabs = dock_tabs(&mut h);
    assert_eq!(tabs.len(), 1);
    assert!(tabs[0].1.contains("exited (127)"), "{tabs:?}");
    assert!(tabs[0].2);
}

fn obs_dock_boxes(h: &mut H) -> (gpui::Bounds<gpui::Pixels>, gpui::Bounds<gpui::Pixels>) {
    (
        h.bounds("obs").expect("obs"),
        h.bounds("term-dock").expect("dock"),
    )
}

fn near(a: gpui::Bounds<gpui::Pixels>, b: gpui::Bounds<gpui::Pixels>) -> f32 {
    f32::from((a.size.height - b.size.height).abs())
        + f32::from((a.size.width - b.size.width).abs())
        + f32::from((a.origin.y - b.origin.y).abs())
        + f32::from((a.origin.x - b.origin.x).abs())
}

#[gpui::test]
async fn ctrl_shift_backtick_maximizes_the_dock_over_the_observatory_and_back(
    cx: &mut TestAppContext,
) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.state_update(|s, cx| s.set_terminal_dock_open(WS, true, cx));
    h.wait(100);
    let (_, normal) = obs_dock_boxes(&mut h);
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, true, cx));
    h.wait(400);
    assert!(h.read(|o, _| o.term_overlaying()));
    let (obs, max) = obs_dock_boxes(&mut h);
    assert!(near(obs, max) < 8., "{obs:?} {max:?}");
    assert!(max.size.height > normal.size.height);
    // The chrome stays in the tree under the overlay.
    assert!(h.has("obs-toolbar") && h.has("obs-side"));
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, false, cx));
    h.wait(400);
    assert!(!h.read(|o, _| o.term_overlaying()));
    let (_, back) = obs_dock_boxes(&mut h);
    assert!(f32::from((back.size.height - normal.size.height).abs()) < 3.);
}

#[gpui::test]
async fn the_grid_keeps_its_size_while_maximize_animates(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.state_update(|s, cx| s.set_terminal_dock_open(WS, true, cx));
    h.wait(100);
    let grid = |h: &mut H| {
        h.read(|o, cx| {
            let d = o.dock.as_ref().expect("dock").1.read(cx);
            d.pane(d.active_id()).expect("pane").read(cx).size()
        })
    };
    let normal = grid(&mut h);
    h.update(|_, _, cx| recur_ui::motion::thaw(cx));
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, true, cx));
    h.wait(100);
    assert!(h.read(|o, _| o.term_max_anim.is_some()));
    assert_eq!(grid(&mut h), normal);
    h.wait(400);
    let max = grid(&mut h);
    assert!(max.1 > normal.1, "{normal:?} {max:?}");
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, false, cx));
    h.wait(100);
    assert_eq!(grid(&mut h), max);
    h.wait(400);
    assert_eq!(grid(&mut h), normal);
}

#[gpui::test]
async fn maximize_and_restore_while_the_dock_is_beside_the_editor(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.click("obs-term-beside-toggle");
    h.wait(100);
    assert!(h.read(|o, cx| o.term_beside_layout(cx)));
    let beside = h.bounds("term-dock").expect("dock");
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, true, cx));
    h.wait(400);
    let (obs, max) = obs_dock_boxes(&mut h);
    assert!(near(obs, max) < 8.);
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, false, cx));
    h.wait(400);
    assert!(h.read(|o, cx| o.term_beside_layout(cx)));
    let back = h.bounds("term-dock").expect("dock");
    assert!(near(back, beside) < 8.);
}

#[gpui::test]
async fn hide_editor_expands_the_terminal_in_the_middle_panel(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    let with_editor = h.bounds("term-dock").expect("dock").size.height;
    h.click("obs-editor-toggle");
    assert!(h.read(|o, cx| o.editor_tucked(cx)));
    assert!(!h.has("obs-editors"));
    assert!(h.has("obs-toolbar") && h.has("obs-side") && h.has("obs-editor-toggle"));
    assert!(!h.read(|o, _| o.term_overlaying()));
    let filled = h.bounds("term-dock").expect("dock").size.height;
    assert!(filled > with_editor);
    h.click("obs-editor-toggle");
    assert!(!h.read(|o, cx| o.editor_tucked(cx)));
    assert!(h.has("obs-editors"));
    let back = h.bounds("term-dock").expect("dock").size.height;
    assert!(f32::from((back - with_editor).abs()) < 3.);
}

#[gpui::test]
async fn hiding_the_editor_opens_the_terminal_when_it_was_closed(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    assert!(!dock_open(&mut h));
    h.click("obs-editor-toggle");
    h.wait(100);
    assert!(dock_open(&mut h));
    assert!(h.read(|o, cx| o.editor_tucked(cx)));
}

#[gpui::test]
// Linux: the second ⌥⌃E reaches the focused terminal, which keeps Ctrl chords (as xterm.js does).
#[cfg_attr(not(target_os = "macos"), ignore)]
async fn opt_cmd_e_toggles_editor_visibility(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.keys("secondary-alt-e");
    h.wait(100);
    assert!(h.read(|o, cx| o.editor_tucked(cx)));
    assert!(dock_open(&mut h));
    h.keys("secondary-alt-e");
    assert!(!h.read(|o, cx| o.editor_tucked(cx)));
}

#[gpui::test]
async fn hiding_the_terminal_clears_editor_hidden(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.click("obs-editor-toggle");
    h.wait(100);
    assert!(h.read(|o, cx| o.editor_tucked(cx)));
    h.click("obs-term-toggle");
    assert!(!dock_open(&mut h));
    assert!(!h.read(|o, cx| o.editor_tucked(cx)));
    assert!(h.has("obs-editors"));
}

#[gpui::test]
async fn hiding_the_dock_clears_maximize(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    // From closed, maximize opens the dock.
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, true, cx));
    h.wait(400);
    assert!(dock_open(&mut h));
    assert!(h.read(|o, _| o.term_overlaying()));
    h.state_update(|s, cx| s.set_terminal_dock_open(WS, false, cx));
    h.wait(400);
    assert!(!dock_open(&mut h));
    h.state_update(|s, cx| s.set_terminal_dock_open(WS, true, cx));
    h.wait(400);
    assert!(dock_open(&mut h));
    assert!(!h.read(|o, _| o.term_overlaying()));
}

#[gpui::test]
async fn dock_visibility_and_maximize_are_per_workspace(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, true, cx));
    h.wait(400);
    assert!(h.read(|o, _| o.term_overlaying()));
    h.switch_to(PROJECT, "LOOP-2229");
    assert!(!dock_open(&mut h));
    assert!(!h.read(|o, _| o.term_overlaying()));
    h.state_update(|s, cx| s.set_terminal_dock_open("LOOP-2229", true, cx));
    h.wait(400);
    assert!(dock_open(&mut h));
    assert!(!h.read(|o, _| o.term_overlaying()));
    h.switch_to(PROJECT, WS);
    h.wait(400);
    assert!(h.read(|o, _| o.term_overlaying()));
}

#[gpui::test]
async fn switching_workspaces_restores_the_last_focused_chrome_not_the_terminal(
    cx: &mut TestAppContext,
) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.keys("secondary-p");
    h.type_text("main.go");
    h.wait(100);
    h.keys("enter");
    h.wait(100);
    assert!(h.active_tab("src/main.go"));
    open_dock(&mut h);
    h.focus_editor();
    assert_eq!(h.read(|o, _| o.focus_zone), FocusZone::Editor);
    let attaches = |t: &Term| t.calls("AttachTerminal").len();
    h.switch_to(PROJECT, "LOOP-2229");
    let before = attaches(&t);
    h.switch_to(PROJECT, WS);
    h.wait(100);
    assert!(h.active_tab("src/main.go"));
    assert!(dock_open(&mut h));
    // Keep-alive: the pane came back from the park, so no new attach.
    assert_eq!(attaches(&t), before);
    assert_eq!(h.read(|o, _| o.focus_zone), FocusZone::Editor);
    let dock_focused = h.cx.update(|window, cx| {
        h.obs
            .read(cx)
            .dock
            .as_ref()
            .is_some_and(|(_, d)| d.read(cx).contains_focus(window, cx))
    });
    assert!(!dock_focused);
    h.click("term-pane:sess-1");
    assert!(term_lit(&mut h));
    h.switch_to(PROJECT, "LOOP-2229");
    h.switch_to(PROJECT, WS);
    h.wait(100);
    assert!(term_lit(&mut h));
}

/// The dock chrome store survives a restart (`ListTerminalDockChrome`).
fn restart_mount<'a>(cx: &'a mut TestAppContext, t: &Term) -> H<'a> {
    mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {})
}

#[gpui::test]
async fn the_dock_survives_a_reload_and_stays_out_of_the_way_when_never_opened(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    assert!(!dock_open(&mut h));
    assert!(t.calls("OpenTerminal").is_empty());
    open_dock(&mut h);
    h.quit();
    let mut h = restart_mount(restarted, &t);
    h.wait(100);
    assert!(dock_open(&mut h));
}

#[gpui::test]
async fn maximize_survives_a_reload(cx: &mut TestAppContext, restarted: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, true, cx));
    h.wait(400);
    assert!(h.read(|o, _| o.term_overlaying()));
    assert!(
        t.calls("SetTerminalDockChrome")
            .iter()
            .any(|a| a[1]["maximized"] == true)
    );
    h.quit();
    let mut h = restart_mount(restarted, &t);
    h.wait(400);
    assert!(dock_open(&mut h));
    assert!(h.read(|o, _| o.term_overlaying()));
}

#[gpui::test]
async fn beside_survives_a_reload_via_dock_chrome(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.click("obs-term-beside-toggle");
    h.wait(100);
    assert!(h.read(|o, cx| o.term_beside_layout(cx)));
    assert!(
        t.calls("SetTerminalDockChrome")
            .iter()
            .any(|a| a[1]["beside"] == true)
    );
    h.quit();
    let mut h = restart_mount(restarted, &t);
    h.wait(100);
    assert!(h.read(|o, cx| o.term_beside_layout(cx)));
    assert!(dock_open(&mut h));
}

fn runs_mount<'a>(cx: &'a mut TestAppContext, t: &Term, runs: &[(&str, &str)]) -> H<'a> {
    mount_seeded(
        cx,
        with_runs(runs),
        t.opts(MockOptions::default()),
        |_| {},
        |_| {},
    )
}

fn dev_running(h: &mut H) -> bool {
    h.state.read_with(h.cx, |s, _| s.run.is_any_run_running(WS))
}

fn writes(t: &Term, pat: &str) -> usize {
    t.calls("WriteTerminal")
        .iter()
        .filter(|a| {
            a.get(1)
                .and_then(Value::as_str)
                .is_some_and(|s| s.contains(pat))
        })
        .count()
}

#[gpui::test]
async fn run_types_into_a_live_tab_stop_interrupts_another_run_reuses_it(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = runs_mount(cx, &t, &[("dev", "pnpm dev --port $RECUR_PORT")]);
    open_dock(&mut h);
    assert!(h.has("term-act:run"));
    h.click("term-act:run");
    h.wait(1000);
    assert!(dev_running(&mut h));
    let last = t.calls("WriteTerminal").last().cloned().expect("write");
    let cmd = last[1].as_str().unwrap_or("").to_string();
    assert!(cmd.contains("pnpm dev") && !cmd.contains("exec"), "{cmd:?}");
    let opens = t.calls("OpenTerminal").len();
    h.click("term-act:run");
    h.wait(1000);
    assert!(!dev_running(&mut h));
    assert!(!t.calls("InterruptTerminal").is_empty());
    assert!(dock_tabs(&mut h).iter().all(|t| !t.2));
    h.click("term-act:run");
    h.wait(1000);
    assert!(dev_running(&mut h));
    assert_eq!(writes(&t, "pnpm dev"), 2);
    assert_eq!(t.calls("OpenTerminal").len(), opens);
}

#[gpui::test]
async fn titlebar_stop_dev_interrupts_the_process_even_if_the_dock_is_hidden(
    cx: &mut TestAppContext,
) {
    let t = Term::default();
    let mut h = runs_mount(cx, &t, &[("dev", "pnpm dev --port $RECUR_PORT")]);
    // The titlebar's Run dev.
    h.state_update(|s, cx| {
        s.request_run_dev(WS, None, cx);
    });
    h.wait(1000);
    assert!(dock_open(&mut h));
    assert!(dev_running(&mut h));
    h.click("term-act:hide");
    assert!(!dock_open(&mut h));
    assert!(dev_running(&mut h));
    let closes = t.calls("CloseTerminal").len();
    // The titlebar's Stop dev.
    h.state_update(|s, cx| {
        s.request_stop_run(WS, "dev", cx);
    });
    h.wait(1000);
    assert!(!dev_running(&mut h));
    assert!(!dock_open(&mut h));
    assert!(!t.calls("InterruptTerminal").is_empty());
    assert_eq!(t.calls("CloseTerminal").len(), closes);
}

#[gpui::test]
async fn an_idle_run_returns_the_controls_to_run_without_killing_the_tab(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = runs_mount(cx, &t, &[("dev", "pnpm dev --port $RECUR_PORT")]);
    open_dock(&mut h);
    h.click("term-act:run");
    h.wait(1000);
    assert!(dev_running(&mut h));
    let n = t.calls("OpenTerminal").len();
    for _ in 0..10 {
        h.event(
            "term.idle",
            json!({"id": format!("sess-{n}"), "workspaceId": WS}),
        );
        h.wait(200);
        if !dev_running(&mut h) {
            break;
        }
    }
    assert!(!dev_running(&mut h));
    assert!(h.has("term-act:run"));
    assert!(dock_tabs(&mut h).iter().all(|t| !t.2));
}

#[gpui::test]
async fn an_exited_run_session_returns_the_controls_to_run(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = runs_mount(cx, &t, &[("dev", "pnpm dev --port $RECUR_PORT")]);
    open_dock(&mut h);
    h.click("term-act:run");
    h.wait(1000);
    assert!(dev_running(&mut h));
    let n = t.calls("OpenTerminal").len();
    h.event(
        "term.exit",
        json!({"id": format!("sess-{n}"), "workspaceId": WS, "exitCode": 0}),
    );
    h.wait(100);
    assert!(!dev_running(&mut h));
}

#[gpui::test]
async fn a_palette_run_opens_the_dock_and_types_the_command(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = runs_mount(
        cx,
        &t,
        &[
            ("dev", "pnpm dev --port $RECUR_PORT"),
            ("test", "go test ./..."),
        ],
    );
    // ⌘R's palette picks `test`; ⌘⇧R's stop palette then stops it.
    h.state_update(|s, cx| {
        let run = recur_domain::RunCommand {
            name: "test".into(),
            cmd: "go test ./...".into(),
        };
        s.request_run_dev(WS, Some(run), cx);
    });
    h.wait(1000);
    assert!(dock_open(&mut h));
    assert_eq!(writes(&t, "go test"), 1);
    h.state_update(|s, cx| {
        s.request_stop_run(WS, "test", cx);
    });
    h.wait(1000);
    assert!(!t.calls("InterruptTerminal").is_empty());
}

/// `TerminalDock.test.ts`: ⌘F opens the find bar, it counts matches, the case toggle narrows
/// them, and Escape closes it.
#[gpui::test]
async fn find_opens_on_cmd_f_counts_matches_and_closes_on_escape(cx: &mut TestAppContext) {
    let t = Term::default();
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    open_dock(&mut h);
    term_out(&mut h, "sess-1", "hello world\r\nHello again\r\n");
    h.wait(100);
    h.click("term-pane:sess-1");
    h.keys(&dock_chord("f"));
    let find = |h: &mut H| {
        h.read(|o, cx| {
            o.dock
                .as_ref()
                .map(|(_, d)| {
                    let d = d.read(cx);
                    (d.find_open, d.find.read(cx).count_label(cx))
                })
                .unwrap_or_default()
        })
    };
    assert!(find(&mut h).0);
    h.type_text("hello");
    h.wait(100);
    assert_eq!(find(&mut h).1, "1 / 2");
    h.keys("secondary-alt-c");
    h.wait(100);
    assert_eq!(find(&mut h).1, "1 / 1");
    h.keys("escape");
    assert!(!find(&mut h).0);
}

/// `animateTermFill` hands keys to the shell (`focusTerminalPanel`): maximizing from the
/// header button lights the dock and focuses the active pane.
#[gpui::test]
async fn the_maximize_button_puts_keys_in_the_active_shell(cx: &mut TestAppContext) {
    let t = Term::default();
    t.chrome.lock().insert(
        WS.into(),
        json!({"workspaceId": WS, "open": true, "maximized": false, "editorHidden": false,
               "beside": false, "besidePct": 0}),
    );
    let mut h = mount(cx, t.opts(MockOptions::default()), |_| {}, |_| {});
    h.wait(100);
    assert!(dock_open(&mut h));
    h.click("term-act:maximize");
    h.wait(600);
    assert!(h.read(|o, _| o.term_overlaying()));
    assert!(term_lit(&mut h));
    let pane_focused = h.update(|o, window, cx| {
        o.dock.as_ref().is_some_and(|(_, d)| {
            let d = d.read(cx);
            d.panes.get(&d.active_id).is_some_and(|p| {
                gpui_kit::Focusable::focus_handle(p.read(cx), cx).is_focused(window)
            })
        })
    });
    assert!(pane_focused);
}
