//! `editor-hotpath.spec.ts`: lazy grammars, the synchronous dirty path and Cmd-S, live
//! refresh versus unsaved edits, tab-switch buffers, session restore across restarts (stashes,
//! caret and scroll, the view chip), Quick Open, read retries, and stash drift warnings.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_editor::view_state::EditorViewMode;
use recur_services::mock::{MockOptions, Reply};
use recur_ui::widgets::ToastKind;
use serde_json::{Value, json};

use super::harness::*;

const MAIN: &str = "package main\n\nfunc main() {}\n";

fn s(a: &[Value], i: usize) -> String {
    a.get(i).and_then(Value::as_str).unwrap_or("").to_string()
}

fn file(path: &str, content: &str) -> Value {
    json!({
        "path": path, "language": "go", "content": content,
        "size": content.len(), "writable": true,
    })
}

type Files = Arc<Mutex<BTreeMap<String, String>>>;

fn files(list: &[(&str, &str)]) -> Files {
    Arc::new(Mutex::new(
        list.iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect(),
    ))
}

/// `goFileResolvers` / `twoFileResolvers`: a `src/` tree over `files`, all changed, with
/// writes persisted.
fn go_files(f: &Files, opts: MockOptions) -> MockOptions {
    let (f1, f2, f3, f4, f5, f6) = (
        f.clone(),
        f.clone(),
        f.clone(),
        f.clone(),
        f.clone(),
        f.clone(),
    );
    opts.resolver("ListTree", move |a, _, _| {
        Reply::value(match s(a, 2).as_str() {
            "" => {
                let root: Vec<Value> = f1
                    .lock()
                    .keys()
                    .filter(|p| !p.contains('/'))
                    .map(|p| json!({"name": p, "path": p, "dir": false}))
                    .collect();
                let mut v = vec![json!({"name": "src", "path": "src", "dir": true})];
                v.extend(root);
                json!(v)
            }
            "src" => json!(
                f1.lock()
                    .keys()
                    .filter(|p| p.starts_with("src/"))
                    .map(|p| json!({"name": &p[4..], "path": p, "dir": false}))
                    .collect::<Vec<_>>()
            ),
            _ => json!([]),
        })
    })
    .resolver("ListChangedFiles", move |_, _, _| {
        Reply::value(json!(f2.lock().keys().cloned().collect::<Vec<_>>()))
    })
    .resolver("GitStatus", move |_, _, _| {
        Reply::value(json!(
            f3.lock()
                .keys()
                .map(|p| (p.clone(), json!("m")))
                .collect::<serde_json::Map<_, _>>()
        ))
    })
    .resolver("ReadFile", move |a, _, _| {
        let p = s(a, 2);
        let body = f4.lock().get(&p).cloned().unwrap_or_else(|| MAIN.into());
        Reply::value(file(&p, &body))
    })
    .resolver("FileAtBase", move |a, _, _| {
        Reply::value(f5.lock().get(&s(a, 2)).cloned().unwrap_or_default())
    })
    .resolver("WriteFile", move |a, _, _| {
        f6.lock().insert(s(a, 2), s(a, 3));
        Reply::null()
    })
}

/// `openFile`: changed files open in Diff; force Code.
fn open_file(h: &mut H, path: &str) {
    h.click(&format!("obs-node:file:{path}"));
    h.wait(50);
    if h.has("view-mode:Code") && h.read(|o, _| o.view_mode) != EditorViewMode::Code {
        h.click("view-mode:Code");
    }
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
}

fn type_at_end(h: &mut H, text: &str) {
    h.focus_editor();
    h.keys("secondary-end");
    h.type_text(text);
}

fn dirty(h: &mut H, path: &str) -> bool {
    let p = path.to_string();
    h.read(move |o, _| o.tab_is_dirty(&p))
}

fn modified(h: &mut H) -> bool {
    h.read(|o, _| o.active_dirty())
}

#[gpui::test]
async fn grammar_loads_lazily_typing_marks_dirty_synchronously_save_clears_it(
    cx: &mut TestAppContext,
) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    assert!(h.editor_doc().contains("func main() {}"));
    assert!(!dirty(&mut h, "src/main.go"));
    let scope = h.read(|o, cx| o.main_editor.as_ref().map(|e| e.read(cx).context().scope));
    assert_eq!(scope.as_deref(), Some("source.go"));
    type_at_end(&mut h, " // edit");
    // Synchronous: no emit debounce needed for the dot.
    assert!(dirty(&mut h, "src/main.go"));
    assert!(modified(&mut h));
    h.keys("secondary-s");
    h.wait(100);
    assert!(!dirty(&mut h, "src/main.go"));
    assert!(h.active_tab("src/main.go"));
}

#[gpui::test]
async fn reverting_a_local_edit_clears_the_unsaved_marker(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "X");
    assert!(dirty(&mut h, "src/main.go"));
    h.keys("backspace");
    assert!(!dirty(&mut h, "src/main.go"));
    assert!(!modified(&mut h));
    assert!(!h.editor_doc().contains('X'));
}

#[gpui::test]
async fn crlf_on_disk_does_not_mark_the_buffer_unsaved(cx: &mut TestAppContext) {
    let opts = MockOptions::default().resolver("ReadFile", |a, _, _| {
        Reply::value(file(&s(a, 2), "package main\r\n\r\nfunc main() {}\r\n"))
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    assert!(h.editor_doc().contains("func main() {}"));
    assert!(!dirty(&mut h, "src/main.go"));
    assert!(!modified(&mut h));
}

#[gpui::test]
async fn the_files_explorer_shows_the_same_unsaved_dot_as_the_editor_tab(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    assert!(!h.has("obs-unsaved:src/main.go"));
    type_at_end(&mut h, " // edit");
    assert!(h.has("obs-unsaved:src/main.go"));
    h.keys("secondary-s");
    h.wait(100);
    assert!(!h.has("obs-unsaved:src/main.go"));
}

#[gpui::test]
async fn save_writes_the_live_buffer_even_when_typed_inside_the_debounce_window(
    cx: &mut TestAppContext,
) {
    let writes = Calls::default();
    let opts = MockOptions::default().resolver("WriteFile", record(&writes, |_| Reply::null()));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    h.focus_editor();
    h.keys("secondary-end");
    h.cx.simulate_input("X");
    h.keys("secondary-s");
    h.wait(100);
    assert!(!dirty(&mut h, "src/main.go"));
    let w = writes.lock().clone();
    assert_eq!(w.len(), 1);
    assert!(s(&w[0], 3).contains('X'));
}

fn live_content() -> (Arc<Mutex<String>>, MockOptions) {
    let content = Arc::new(Mutex::new(MAIN.to_string()));
    let c = content.clone();
    let opts = MockOptions::default()
        .resolver("ReadFile", move |a, _, _| {
            Reply::value(file(&s(a, 2), &c.lock()))
        })
        .resolver("FileAtBase", |_, _, _| Reply::value(MAIN));
    (content, opts)
}

#[gpui::test]
async fn an_external_on_disk_change_refreshes_a_clean_buffer(cx: &mut TestAppContext) {
    let (content, opts) = live_content();
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    *content.lock() = "package main\n\nfunc main() { AGENT_EDIT }\n".into();
    h.fs_changed();
    h.wait(100);
    assert!(h.editor_doc().contains("AGENT_EDIT"));
}

fn merge_doc(h: &mut H) -> String {
    h.read(|o, cx| {
        o.merge_editor
            .as_ref()
            .map(|(_, e)| e.read(cx).peek_doc())
            .unwrap_or_default()
    })
}

#[gpui::test]
async fn diff_rehydrates_when_a_deleted_file_comes_back_on_disk(cx: &mut TestAppContext) {
    let status = Arc::new(Mutex::new(json!({"src/gone.go": "d"})));
    let content: Arc<Mutex<Option<String>>> = Arc::default();
    let (s1, s2, c1, c2) = (
        status.clone(),
        status.clone(),
        content.clone(),
        content.clone(),
    );
    let opts = MockOptions::default()
        .resolver("ListTree", move |a, _, _| {
            Reply::value(match s(a, 2).as_str() {
                "" => json!([{"name": "src", "path": "src", "dir": true}]),
                "src" if c1.lock().is_some() => {
                    json!([{"name": "gone.go", "path": "src/gone.go", "dir": false}])
                }
                _ => json!([]),
            })
        })
        .resolver("ListChangedFiles", |_, _, _| {
            Reply::value(json!(["src/gone.go"]))
        })
        .resolver("GitStatus", move |_, _, _| Reply::value(s1.lock().clone()))
        .resolver("WorkingStatus", move |_, _, _| {
            Reply::value(s2.lock().clone())
        })
        .resolver("FileAtBase", |_, _, _| Reply::value(MAIN))
        .resolver("ReadFile", move |a, _, _| match c2.lock().clone() {
            None => Reply::error("src/gone.go: no such file or directory"),
            Some(c) => Reply::value(file(&s(a, 2), &c)),
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:src/gone.go");
    h.wait(100);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
    *content.lock() = Some("package main\n\nfunc main() { RESTORED }\n".into());
    *status.lock() = json!({"src/gone.go": "m"});
    h.fs_changed();
    h.wait(200);
    assert!(merge_doc(&mut h).contains("RESTORED"));
}

#[gpui::test]
async fn an_external_on_disk_change_refreshes_a_clean_diff_buffer(cx: &mut TestAppContext) {
    let (content, opts) = live_content();
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:src/main.go");
    h.wait(100);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(merge_doc(&mut h).contains("func main() {}"));
    *content.lock() = "package main\n\nfunc main() { AGENT_EDIT }\n".into();
    h.fs_changed();
    h.wait(100);
    assert!(merge_doc(&mut h).contains("AGENT_EDIT"));
}

#[gpui::test]
async fn an_external_on_disk_change_must_not_clobber_unsaved_local_edits(cx: &mut TestAppContext) {
    let (content, opts) = live_content();
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "LOCAL");
    *content.lock() = "package main\n\nfunc main() { AGENT_EDIT }\n".into();
    h.fs_changed();
    h.wait(1200);
    let doc = h.editor_doc();
    assert!(
        doc.contains("LOCAL") && !doc.contains("AGENT_EDIT"),
        "{doc:?}"
    );
}

fn two() -> Files {
    files(&[("src/main.go", MAIN), ("src/other.go", "package other\n")])
}

fn click_tab(h: &mut H, path: &str) {
    let p = path.to_string();
    h.update(move |o, window, cx| o.open_path(crate::OpenRequest::new(p), window, cx).detach());
    h.wait(50);
    if h.has("view-mode:Code") && h.read(|o, _| o.view_mode) != EditorViewMode::Code {
        h.click("view-mode:Code");
    }
}

#[gpui::test]
async fn tab_switch_restores_the_saved_buffer(cx: &mut TestAppContext) {
    let f = two();
    let mut h = mount(cx, go_files(&f, MockOptions::default()), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "KEEPME");
    h.keys("secondary-s");
    h.wait(100);
    assert!(!dirty(&mut h, "src/main.go"));
    open_file(&mut h, "src/other.go");
    assert!(h.editor_doc().contains("package other"));
    click_tab(&mut h, "src/main.go");
    assert!(h.editor_doc().contains("KEEPME"));
}

#[gpui::test]
async fn unsaved_edits_survive_a_tab_switch_without_discarding(cx: &mut TestAppContext) {
    let f = two();
    let mut h = mount(cx, go_files(&f, MockOptions::default()), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "KEEPME");
    assert!(dirty(&mut h, "src/main.go"));
    open_file(&mut h, "src/other.go");
    let doc = h.editor_doc();
    assert!(doc.contains("package other") && !doc.contains("KEEPME"));
    assert!(dirty(&mut h, "src/main.go"));
    click_tab(&mut h, "src/main.go");
    assert!(h.editor_doc().contains("KEEPME"));
    assert!(dirty(&mut h, "src/main.go"));
}

/// Quit after the debounced session patch lands, then remount in `restarted` over the same
/// files and session store.
fn restart<'a>(h: H, restarted: &'a mut TestAppContext, f: &Files, store: &SessionStore) -> H<'a> {
    let mut h = h;
    h.wait(600);
    h.quit();
    let mut h = mount(
        restarted,
        store.install(go_files(f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    h.wait(200);
    h
}

#[gpui::test]
async fn unsaved_edits_survive_an_app_restart(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = two();
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "KEEPME");
    assert!(dirty(&mut h, "src/main.go"));
    let mut h = restart(h, restarted, &f, &store);
    if h.read(|o, _| o.view_mode) != EditorViewMode::Code {
        h.click("view-mode:Code");
    }
    assert!(h.editor_doc().contains("KEEPME"));
    assert!(dirty(&mut h, "src/main.go"));
}

#[gpui::test]
async fn caret_and_scroll_survive_an_app_restart(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let long: String = (1..=80).map(|i| format!("// line {i}\n")).collect();
    let f = files(&[("src/main.go", &long)]);
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    open_file(&mut h, "src/main.go");
    h.focus_editor();
    // Document end: ⌘↓ is CodeMirror's macOS binding, Ctrl+End elsewhere.
    h.keys(if cfg!(target_os = "macos") {
        "cmd-down"
    } else {
        "ctrl-end"
    });
    let line = h.read(|o, _| o.cursor_line);
    assert!(line == 80 || line == 81, "{line}");
    let top = h.read(|o, cx| {
        o.main_editor
            .as_ref()
            .map_or(0., |e| e.read(cx).scroll_top())
    });
    assert!(top > 0.);
    let mut h = restart(h, restarted, &f, &store);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    let line = h.read(|o, _| o.cursor_line);
    assert!(line == 80 || line == 81, "{line}");
    let top = h.read(|o, cx| {
        o.main_editor
            .as_ref()
            .map_or(0., |e| e.read(cx).scroll_top())
    });
    assert!(top > 0.);
}

#[gpui::test]
async fn the_code_chip_survives_an_app_restart(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = files(&[("src/main.go", MAIN)]);
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    open_file(&mut h, "src/main.go");
    let mut h = restart(h, restarted, &f, &store);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    assert!(h.read(|o, _| o.main_editor.is_some()));
}

#[gpui::test]
async fn the_diff_chip_survives_an_app_restart(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = files(&[("src/main.go", MAIN)]);
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    h.click("obs-node:file:src/main.go");
    h.wait(100);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    let mut h = restart(h, restarted, &f, &store);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
}

#[gpui::test]
async fn the_diff_chip_survives_restart_when_git_status_lags_the_session(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = files(&[("src/main.go", MAIN)]);
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    h.click("obs-node:file:src/main.go");
    h.wait(100);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    h.wait(600);
    h.quit();
    // GitStatus answers late after the restart.
    let (reply, release) = Reply::deferred();
    let slot = Arc::new(Mutex::new(Some(reply)));
    let opts = store
        .install(go_files(&f, MockOptions::default()))
        .resolver("GitStatus", move |_, _, _| {
            slot.lock()
                .take()
                .unwrap_or_else(|| Reply::value(json!({"src/main.go": "m"})))
        });
    let mut h = mount(restarted, opts, |_| {}, |_| {});
    h.wait(200);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    release.send(Reply::value(json!({"src/main.go": "m"})));
    h.wait(700);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
}

#[gpui::test]
async fn the_preview_chip_survives_an_app_restart(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = files(&[("README.md", "# Hello\n\nWorld\n")]);
    let store = SessionStore::default();
    let opts = |o: MockOptions| {
        go_files(&f, o).resolver("FileAtBase", |_, _, _| Reply::value("# Hello\n"))
    };
    let mut h = mount(
        cx,
        store.install(opts(MockOptions::default())),
        |_| {},
        |_| {},
    );
    h.click("obs-node:file:README.md");
    h.wait(50);
    h.click("view-mode:Preview");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Preview);
    assert!(h.read(|o, _| o.file_preview.is_some()));
    h.wait(600);
    h.quit();
    let mut h = mount(
        restarted,
        store.install(opts(MockOptions::default())),
        |_| {},
        |_| {},
    );
    h.wait(200);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Preview);
    assert!(h.read(|o, _| o.file_preview.is_some()));
}

#[gpui::test]
async fn quick_open_filters_the_file_index(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    h.keys("secondary-p");
    assert!(h.has("quick-open"));
    h.type_text("main");
    h.wait(100);
    assert!(h.has("qo-row:0"));
    h.type_text("zzzzz");
    h.wait(100);
    assert!(!h.has("qo-row:0"));
}

#[gpui::test]
async fn quick_open_hydrates_the_picked_file_without_a_read_failed_toast(cx: &mut TestAppContext) {
    let f = two();
    let f2 = f.clone();
    let opts = go_files(&f, MockOptions::default()).resolver("ListFiles", move |_, _, _| {
        Reply::value(json!(f2.lock().keys().cloned().collect::<Vec<_>>()))
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    // Opening a file must not steal the side panel.
    h.click("panel-toggle:search");
    assert_eq!(
        h.read(|o, _| o.right_kind),
        recur_view_core::prefs::CodePanelKey::Search
    );
    h.keys("secondary-p");
    h.type_text("other");
    h.wait(100);
    h.keys("enter");
    h.wait(100);
    assert!(!h.has("quick-open"));
    assert!(h.active_tab("src/other.go"));
    h.click("view-mode:Code");
    assert!(h.editor_doc().contains("package other"));
    assert_eq!(
        h.read(|o, _| o.right_kind),
        recur_view_core::prefs::CodePanelKey::Search
    );
    assert!(!h.has_toast(ToastKind::Error, "Read failed"));
}

#[gpui::test]
async fn quick_open_scrolls_the_explorer_to_the_selected_file_when_files_is_showing(
    cx: &mut TestAppContext,
) {
    let mut list = vec![("src/main.go".to_string(), MAIN.to_string())];
    for i in 0..40 {
        list.push((format!("src/f{i:02}.go"), format!("package f{i}\n")));
    }
    let f: Files = Arc::new(Mutex::new(list.into_iter().collect()));
    let f2 = f.clone();
    let opts = go_files(&f, MockOptions::default()).resolver("ListFiles", move |_, _, _| {
        Reply::value(json!(f2.lock().keys().cloned().collect::<Vec<_>>()))
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    assert_eq!(
        h.read(|o, _| o.right_kind),
        recur_view_core::prefs::CodePanelKey::Files
    );
    let last = "src/f39.go";
    let visible = |h: &mut H| {
        let pane = h.bounds("obs-files");
        let row = h.bounds(&format!("obs-node:file:{last}"));
        match (pane, row) {
            (Some(p), Some(r)) => {
                r.top() >= p.top() - gpui::px(1.) && r.bottom() <= p.bottom() + gpui::px(1.)
            }
            _ => false,
        }
    };
    assert!(!visible(&mut h));
    for _ in 0..2 {
        h.keys("secondary-p");
        h.type_text("f39");
        h.wait(100);
        h.keys("enter");
        h.wait(100);
        assert_eq!(h.read(|o, _| o.tree_path.clone()), last);
        assert!(visible(&mut h));
        // Re-picking the already-open file still scrolls.
        h.update(|o, _, _| {
            o.files_scroll
                .set_offset(gpui::point(gpui::px(0.), gpui::px(0.)))
        });
        assert!(!visible(&mut h));
    }
}

#[gpui::test]
async fn a_single_keystroke_reaches_diff_after_the_emit_debounce(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "X");
    assert!(dirty(&mut h, "src/main.go"));
    h.wait(200);
    h.click("view-mode:Diff");
    assert!(merge_doc(&mut h).contains('X'));
}

#[gpui::test]
async fn reverting_a_restored_stash_clears_the_unsaved_marker(cx: &mut TestAppContext) {
    let f = two();
    let mut h = mount(cx, go_files(&f, MockOptions::default()), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "X");
    assert!(dirty(&mut h, "src/main.go"));
    open_file(&mut h, "src/other.go");
    assert!(dirty(&mut h, "src/main.go"));
    click_tab(&mut h, "src/main.go");
    assert!(h.editor_doc().contains('X'));
    h.focus_editor();
    h.keys("secondary-end");
    h.keys("backspace");
    assert!(!dirty(&mut h, "src/main.go"));
    assert!(!modified(&mut h));
}

#[gpui::test]
async fn closing_the_last_tab_does_not_restore_it_after_restart(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = files(&[("src/main.go", MAIN)]);
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    open_file(&mut h, "src/main.go");
    h.click("tab-close:file-tab:src/main.go");
    assert!(!h.has_tab("src/main.go"));
    let mut h = restart(h, restarted, &f, &store);
    assert!(!h.has_tab("src/main.go"));
    assert!(h.read(|o, _| o.editor_path.is_empty()));
}

#[gpui::test]
async fn re_clicking_a_file_retries_after_a_failed_read(cx: &mut TestAppContext) {
    let reads = Arc::new(Mutex::new(0));
    let r = reads.clone();
    let opts = MockOptions::default().resolver("ReadFile", move |a, _, _| {
        let mut n = r.lock();
        *n += 1;
        if *n == 1 {
            return Reply::error("binary or non-UTF-8 file");
        }
        Reply::value(file(&s(a, 2), "package main\n\nfunc main() { RETRY_OK }\n"))
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:src/main.go");
    h.wait(100);
    assert!(h.has_toast(ToastKind::Error, "Read failed"));
    // Code stays disabled; Diff mounts against the base, not a stuck overlay.
    assert!(h.read(|o, _| o.file.is_none()));
    assert!(h.read(|o, _| o.merge_editor.is_some()));
    assert!(!h.has("obs-load-overlay"));
    h.click("obs-node:file:src/main.go");
    h.wait(100);
    h.click("view-mode:Code");
    assert!(h.editor_doc().contains("RETRY_OK"));
}

#[gpui::test]
async fn restoring_a_stash_warns_when_disk_changed_while_the_tab_was_away(cx: &mut TestAppContext) {
    let f = two();
    let mut h = mount(cx, go_files(&f, MockOptions::default()), |_| {}, |_| {});
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "KEEPME");
    open_file(&mut h, "src/other.go");
    f.lock().insert(
        "src/main.go".into(),
        "package main\n\nfunc main() { AGENT_EDIT }\n".into(),
    );
    click_tab(&mut h, "src/main.go");
    assert!(h.has_toast(ToastKind::Info, "changed on disk"));
    let doc = h.editor_doc();
    assert!(doc.contains("KEEPME") && !doc.contains("AGENT_EDIT"));
    h.focus_editor();
    h.keys("secondary-end");
    for _ in 0.."KEEPME".len() {
        h.keys("backspace");
    }
    // Back to the stash-time disk text, but disk moved on.
    assert!(dirty(&mut h, "src/main.go"));
    assert!(modified(&mut h));
}

#[gpui::test]
async fn restoring_a_stash_after_a_restart_warns_when_disk_moved_meanwhile(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = two();
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "KEEPME");
    h.wait(600);
    h.quit();
    f.lock().insert(
        "src/main.go".into(),
        "package main\n\nfunc main() { AGENT_EDIT }\n".into(),
    );
    let mut h = mount(
        restarted,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    h.wait(200);
    assert!(h.has_toast(ToastKind::Info, "changed on disk"));
    if h.read(|o, _| o.view_mode) != EditorViewMode::Code {
        h.click("view-mode:Code");
    }
    let doc = h.editor_doc();
    assert!(doc.contains("KEEPME") && !doc.contains("AGENT_EDIT"));
    assert!(dirty(&mut h, "src/main.go"));
}

#[gpui::test]
async fn opening_a_restored_stash_in_the_split_pane_warns_when_disk_moved(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let f = two();
    let store = SessionStore::default();
    let mut h = mount(
        cx,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    open_file(&mut h, "src/other.go");
    type_at_end(&mut h, "OTHER");
    open_file(&mut h, "src/main.go");
    type_at_end(&mut h, "KEEPME");
    h.wait(600);
    h.quit();
    f.lock().insert(
        "src/other.go".into(),
        "package other\n\nfunc Other() { AGENT_EDIT }\n".into(),
    );
    let mut h = mount(
        restarted,
        store.install(go_files(&f, MockOptions::default())),
        |_| {},
        |_| {},
    );
    h.wait(200);
    assert!(h.has_tab("src/other.go"));
    if h.read(|o, _| o.view_mode) != EditorViewMode::Code {
        h.click("view-mode:Code");
    }
    h.focus_editor();
    h.keys("secondary-\\");
    assert!(h.read(|o, _| o.split_open));
    h.click("obs-editor-pane:split");
    click_tab(&mut h, "src/other.go");
    assert!(h.has_toast(ToastKind::Info, "changed on disk"));
    let doc = h.split_doc();
    assert!(
        doc.contains("OTHER") && !doc.contains("AGENT_EDIT"),
        "{doc:?}"
    );
}

/// `FilePreview.test.ts` ("does not intercept ⌘F when ... a CodeMirror editor owns the event"):
/// with the preview in the main pane and the same file's code in the split, ⌘F from the split
/// editor opens the editor's search and leaves the preview's find bar closed.
#[gpui::test]
async fn cmd_f_in_the_split_editor_stays_out_of_the_preview_find(cx: &mut TestAppContext) {
    let f = files(&[("README.md", "# Hello\n\nneedle\n")]);
    let mut h = mount(cx, go_files(&f, MockOptions::default()), |_| {}, |_| {});
    h.click("obs-node:file:README.md");
    h.wait(50);
    if h.read(|o, _| o.view_mode) != EditorViewMode::Code {
        h.click("view-mode:Code");
    }
    h.click("crumb:split");
    assert!(h.read(|o, _| o.split_open));
    h.click("view-mode:Preview");
    assert!(h.read(|o, _| o.file_preview.is_some()));
    let split = h.read(|o, _| o.split_editor.clone()).expect("split editor");
    h.update(|_, window, cx| {
        let f = gpui_kit::Focusable::focus_handle(split.read(cx), cx);
        window.focus(&f, cx);
    });
    h.keys("secondary-f");
    let (editor_find, preview_find) = h.read(|o, cx| {
        (
            o.split_editor
                .as_ref()
                .is_some_and(|e| e.read(cx).is_search_open()),
            o.file_preview
                .as_ref()
                .is_some_and(|(_, p)| p.read(cx).is_search_open()),
        )
    });
    assert!(editor_find);
    assert!(!preview_find);
}
