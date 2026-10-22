//! `observatory.spec.ts`. The project-setup and inbox / Briefing scenarios belong to the
//! views that host them; their Observatory halves (occluder, badges) are here. The review
//! surface's own scenarios live in `recur_view_review::spec::observatory`.

use gpui::TestAppContext;
use recur_editor::view_state::EditorViewMode;
use recur_services::mock::{MockOptions, Reply};
use recur_view_core::prefs::CodePanelKey;
use serde_json::{Value, json};

use super::harness::*;
use crate::model::MainSurface;

fn s(a: &[Value], i: usize) -> String {
    a.get(i).and_then(Value::as_str).unwrap_or("").to_string()
}

fn file(path: &str, language: &str, content: &str, writable: bool) -> Value {
    json!({
        "path": path, "language": language, "content": content,
        "size": content.len(), "writable": writable,
    })
}

/// The mock's tree: `src/main.go`, changed (`m`) versus base.
fn open_main_go(h: &mut H) {
    h.click("obs-node:file:src/main.go");
}

#[gpui::test]
async fn loop_detail_mounts_the_observatory(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    assert!(h.has("obs"));
    assert!(h.has("obs-left"));
    assert!(h.has("agent-chat"));
    // No unread on the Agent toggle.
    let n = h.read(|o, cx| o.pane_badge_count(CodePanelKey::Maestro, cx));
    assert_eq!(n, 0);
}

#[gpui::test]
async fn agent_chat_icon_shows_an_unread_badge_from_workspace_attention(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    // Agent on screen: the transcript is not occluded.
    let occluded = h.state.read_with(h.cx, |s, _| {
        s.overlays.chat_occluders.contains("agent-pane")
    });
    assert!(!occluded);
    // Another left panel: the agent pane is occluded so unread can land.
    h.click("panel-toggle:findings");
    let occluded = h.state.read_with(h.cx, |s, _| {
        s.overlays.chat_occluders.contains("agent-pane")
    });
    assert!(occluded);
    h.bridge.simulate_chat_turn_idle(WS, "MA");
    h.settle();
    let title = h.read(|o, cx| o.pane_badge_title(CodePanelKey::Maestro, cx));
    assert_eq!(title, "Agent (1 unread)");
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Maestro, cx)),
        1
    );
}

#[gpui::test]
async fn agent_chat_unread_badges_when_the_observatory_is_off_screen(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    // Maximizing the terminal (the shell's ⌃⇧`) hides the agent dock: the transcript is
    // occluded.
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, true, cx));
    assert!(h.read(|o, _| o.term_overlaying()));
    assert!(h.state.read_with(h.cx, |s, _| {
        s.overlays.chat_occluders.contains("agent-pane")
    }));
    h.state_update(|s, cx| s.set_terminal_dock_maximized(WS, false, cx));
    h.wait(400);
    assert!(!h.read(|o, _| o.term_overlaying()));
    assert!(!h.state.read_with(h.cx, |s, _| {
        s.overlays.chat_occluders.contains("agent-pane")
    }));
}

#[gpui::test]
async fn loop_code_tab_opens_the_editor_for_a_changed_file(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_main_go(&mut h);
    // Changed files open in Diff; switch to Code.
    h.click("view-mode:Code");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    assert!(h.active_tab("src/main.go"));
    assert!(h.editor_doc().contains("package main"));
    assert!(h.has("obs-side"));
}

#[gpui::test]
async fn save_keeps_the_file_tab_selected(cx: &mut TestAppContext) {
    let writes = Calls::default();
    let opts = MockOptions::default().resolver("WriteFile", record(&writes, |_| Reply::null()));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_main_go(&mut h);
    h.click("view-mode:Code");
    h.focus_editor();
    h.keys("cmd-end");
    h.type_text(" // edit");
    h.wait(300);
    assert!(h.read(|o, _| o.tab_is_dirty("src/main.go")));
    h.keys("cmd-s");
    h.wait(300);
    assert!(h.active_tab("src/main.go"));
    assert!(!h.read(|o, _| o.tab_is_dirty("src/main.go")));
    let w = writes.lock().clone();
    assert_eq!(w.len(), 1);
    assert!(s(&w[0], 3).contains("}\n // edit"), "wrote {:?}", w[0]);
}

#[gpui::test]
async fn opening_a_folder_keeps_the_open_file_and_breadcrumbs(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_main_go(&mut h);
    h.click("view-mode:Code");
    assert!(h.active_tab("src/main.go"));
    assert!(h.has("crumb:1"));
    assert!(h.editor_doc().contains("package main"));
    h.click("obs-node:dir:src");
    assert_eq!(h.read(|o, _| o.tree_path.clone()), "src");
    assert!(h.active_tab("src/main.go"));
    assert!(h.has("crumb:1"));
    assert!(h.editor_doc().contains("package main"));
}

#[gpui::test]
async fn double_clicking_a_file_or_folder_starts_a_rename(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    h.dblclick("obs-node:file:src/main.go");
    assert!(h.has("obs-node-draft"));
    let name = h.read(|o, cx| o.draft_edit.as_ref().map(|d| d.name(cx)));
    assert_eq!(name.as_deref(), Some("main.go"));
    h.keys("escape");
    assert!(!h.has("obs-node-draft"));
    h.dblclick("obs-node:dir:src");
    let name = h.read(|o, cx| o.draft_edit.as_ref().map(|d| d.name(cx)));
    assert_eq!(name.as_deref(), Some("src"));
}

fn notes_tree()
-> impl Fn(&[Value], &mut recur_services::mock::Seed, &recur_services::mock::Table) -> Reply
+ Send
+ Sync
+ 'static {
    |a, _, _| {
        Reply::value(match s(a, 2).as_str() {
            "" => json!([
                {"name": "notes.txt", "path": "notes.txt", "dir": false},
                {"name": "src", "path": "src", "dir": true},
            ]),
            "src" => json!([{"name": "main.go", "path": "src/main.go", "dir": false}]),
            _ => json!([]),
        })
    }
}

#[gpui::test]
async fn dragging_an_explorer_row_onto_a_folder_moves_it_there(cx: &mut TestAppContext) {
    let renames = Calls::default();
    let opts = MockOptions::default()
        .resolver("ListTree", notes_tree())
        .resolver("Rename", record(&renames, |_| Reply::null()));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-scope:all");
    h.drag("obs-node:file:notes.txt", "obs-node:dir:src");
    let r: Vec<(String, String)> = renames.lock().iter().map(|a| (s(a, 2), s(a, 3))).collect();
    assert_eq!(r, vec![("notes.txt".into(), "src/notes.txt".into())]);
    // A folder onto a file's row: the row's folder is the root, where it already is.
    h.drag("obs-node:dir:src", "obs-node:file:notes.txt");
    assert_eq!(renames.lock().len(), 1);
}

#[gpui::test]
async fn double_click_rename_stays_focused_after_a_slow_all_scope_open(cx: &mut TestAppContext) {
    let opts = MockOptions::default()
        .resolver("ListTree", notes_tree())
        .resolver("ReadFile", |a, _, _| {
            let path = s(a, 2);
            let body = if path == "notes.txt" {
                "hello notes\n"
            } else {
                "package main\n\nfunc main() {}\n"
            };
            Reply::value(file(
                &path,
                if path.ends_with(".txt") { "text" } else { "go" },
                body,
                !s(a, 1).is_empty(),
            ))
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-scope:all");
    h.dblclick("obs-node:file:notes.txt");
    assert!(h.has("obs-node-draft"));
    h.wait(600);
    assert!(h.editor_doc().contains("hello notes"));
    assert!(h.has("obs-node-draft"));
    let focused = h.cx.update(|window, cx| {
        h.obs.read(cx).draft_edit.as_ref().is_some_and(|d| {
            gpui_kit::Focusable::focus_handle(d.input.read(cx), cx).is_focused(window)
        })
    });
    assert!(focused, "rename field lost focus to the editor");
}

#[gpui::test]
async fn changed_files_list_updates_live_as_the_agent_edits(cx: &mut TestAppContext) {
    let changed = std::sync::Arc::new(parking_lot::Mutex::new(vec!["src/main.go".to_string()]));
    let c2 = changed.clone();
    let opts = MockOptions::default().resolver("ListChangedFiles", move |_, _, _| {
        Reply::value(json!(*c2.lock()))
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    assert_eq!(h.read(|o, cx| o.review_paths(cx).len()), 1);
    assert!(h.has("obs-node:file:src/main.go"));
    *changed.lock() = vec!["src/main.go".into(), "src/new_feature.go".into()];
    h.fs_changed();
    h.wait(100);
    assert_eq!(h.read(|o, cx| o.review_paths(cx).len()), 2);
    assert!(h.has("obs-node:file:src/new_feature.go"));
}

#[gpui::test]
async fn an_open_file_rereads_live_when_the_agent_edits_it_on_disk(cx: &mut TestAppContext) {
    let content = std::sync::Arc::new(parking_lot::Mutex::new(
        "package main\n\nfunc main() {}\n".to_string(),
    ));
    let c2 = content.clone();
    let opts = MockOptions::default().resolver("ReadFile", move |a, _, _| {
        let path = s(a, 2);
        Reply::value(file(&path, "go", &c2.lock(), !s(a, 1).is_empty()))
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_main_go(&mut h);
    h.click("view-mode:Code");
    assert!(h.editor_doc().contains("func main() {}"));
    *content.lock() = "package main\n\nfunc main() {\n\tprintln(\"patched by agent\")\n}\n".into();
    h.fs_changed();
    h.wait(100);
    assert!(h.editor_doc().contains("patched by agent"));
    assert!(h.active_tab("src/main.go"));
}

#[gpui::test]
async fn cmd_shift_v_cycles_the_view_chip_through_diff_and_code(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    open_main_go(&mut h);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
    h.keys("cmd-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    h.keys("cmd-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
}

/// A 1×1 PNG; the current revision differs from the base in its last byte.
const PNG: [u8; 69] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x08, 0xd7, 0x63, 0xf8, 0xff, 0xff, 0x3f,
    0x00, 0x05, 0xfe, 0x02, 0xfe, 0xdc, 0xcc, 0x59, 0xe7, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];

fn png_asset(a: &[Value]) -> Reply {
    use base64::Engine as _;
    let url = s(a, 0);
    let mime = if url.contains(".mp4") {
        "video/mp4"
    } else {
        "image/png"
    };
    Reply::value(json!({
        "contentType": mime,
        "body": base64::engine::general_purpose::STANDARD.encode(PNG),
    }))
}

#[gpui::test]
async fn binary_images_skip_code_and_modified_ones_offer_preview_and_diff(cx: &mut TestAppContext) {
    let status =
        json!({"src/main.go": "m", "src/shot.png": "u", "src/icon.png": "m", "src/gone.png": "d"});
    let (st1, st2) = (status.clone(), status.clone());
    let opts = MockOptions::default()
        .resolver("ListTree", |a, _, _| {
            Reply::value(match s(a, 2).as_str() {
                "" => json!([{"name": "src", "path": "src", "dir": true}]),
                "src" => json!([
                    {"name": "main.go", "path": "src/main.go", "dir": false},
                    {"name": "shot.png", "path": "src/shot.png", "dir": false},
                    {"name": "icon.png", "path": "src/icon.png", "dir": false},
                    {"name": "gone.png", "path": "src/gone.png", "dir": false},
                ]),
                _ => json!([]),
            })
        })
        .resolver("ListChangedFiles", |_, _, _| {
            Reply::value(json!([
                "src/main.go",
                "src/shot.png",
                "src/icon.png",
                "src/gone.png"
            ]))
        })
        .resolver("GitStatus", move |_, _, _| Reply::value(st1.clone()))
        .resolver("WorkingStatus", move |_, _, _| Reply::value(st2.clone()))
        .resolver("ReadFile", |a, _, _| {
            let path = s(a, 2);
            if path.ends_with(".png") {
                return Reply::value(json!({
                    "path": path, "language": "", "content": "", "size": PNG.len(),
                    "writable": !s(a, 1).is_empty(), "mime": "image/png",
                }));
            }
            Reply::value(file(&path, "go", "package main\n\nfunc main() {}\n", true))
        })
        .resolver("FetchAsset", |a, _, _| png_asset(a));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:src/shot.png");
    assert!(h.has("obs-image:current"));
    assert!(!h.has("view-mode:Code"));
    assert!(!h.has("obs-image:base"));
    assert!(!h.read(|o, _| o.can_toggle_find()));
    // Click the raster: the lightbox shows the workspace file.
    h.click("obs-image:current");
    let src = h.read(|o, cx| o.media_zoom.as_ref().map(|l| l.read(cx).src().to_string()));
    assert!(
        src.as_deref()
            .is_some_and(|s| s.contains("path=src%2Fshot.png")),
        "{src:?}"
    );
    h.keys("escape");
    assert!(h.read(|o, _| o.media_zoom.is_none()));

    h.click("obs-node:file:src/icon.png");
    assert!(h.has("view-mode:Preview"));
    assert!(h.has("view-mode:Diff"));
    assert!(!h.has("view-mode:Code"));
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.has("obs-image:base"));
    assert!(h.has("obs-image:current"));
    assert!(h.read(|o, _| o.merge_editor.is_none()));
    assert!(!h.read(|o, _| o.can_toggle_find()));

    h.click("view-mode:Preview");
    assert!(!h.has("obs-image:base"));
    assert!(h.has("obs-image:current"));

    h.click("obs-node:file:src/gone.png");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(!h.has("view-mode:Preview"));
    assert!(h.has("obs-image:base"));
    assert!(!h.read(|o, _| o.can_toggle_find()));
}

#[gpui::test]
async fn videos_show_in_preview_and_zoom_opens_externally(cx: &mut TestAppContext) {
    let opened = Calls::default();
    let opts = MockOptions::default()
        .resolver("ListTree", |a, _, _| {
            Reply::value(if s(a, 2).is_empty() {
                json!([{"name": "clip.mp4", "path": "clip.mp4", "dir": false}])
            } else {
                json!([])
            })
        })
        .resolver("ListChangedFiles", |_, _, _| {
            Reply::value(json!(["clip.mp4"]))
        })
        .resolver("GitStatus", |_, _, _| {
            Reply::value(json!({"clip.mp4": "u"}))
        })
        .resolver("WorkingStatus", |_, _, _| {
            Reply::value(json!({"clip.mp4": "u"}))
        })
        .resolver("ReadFile", |a, _, _| {
            Reply::value(json!({
                "path": s(a, 2), "language": "", "content": "", "size": 2048,
                "writable": true, "mime": "video/mp4",
            }))
        })
        .resolver("OpenInEditor", record(&opened, |_| Reply::null()));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:clip.mp4");
    assert!(h.has("obs-video"));
    assert!(!h.has("view-mode:Code"));
    // GPUI has no media element: the zoom opens the clip in the system player.
    h.click("video-zoom");
    assert_eq!(opened.lock().len(), 1);
}

#[gpui::test]
async fn cmd_shift_v_cycles_code_diff_and_preview(cx: &mut TestAppContext) {
    let opts = MockOptions::default()
        .resolver("ListTree", |a, _, _| {
            Reply::value(match s(a, 2).as_str() {
                "" => json!([
                    {"name": "README.md", "path": "README.md", "dir": false},
                    {"name": "src", "path": "src", "dir": true},
                ]),
                "src" => json!([{"name": "main.go", "path": "src/main.go", "dir": false}]),
                _ => json!([]),
            })
        })
        .resolver("ListChangedFiles", |_, _, _| {
            Reply::value(json!(["src/main.go", "README.md"]))
        })
        .resolver("GitStatus", |_, _, _| {
            Reply::value(json!({"src/main.go": "m", "README.md": "m"}))
        })
        .resolver("WorkingStatus", |_, _, _| {
            Reply::value(json!({"src/main.go": "m", "README.md": "m"}))
        })
        .resolver("ReadFile", |a, _, _| {
            let path = s(a, 2);
            Reply::value(if path.ends_with(".md") {
                file(&path, "markdown", "# Hello\n\nWorld\n", true)
            } else {
                file(&path, "go", "package main\n\nfunc main() {}\n", true)
            })
        })
        .resolver("FileAtBase", |a, _, _| {
            Reply::value(if s(a, 2).ends_with(".md") {
                "# Hello\n"
            } else {
                "package main\n\nfunc main() {}\n"
            })
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:README.md");
    assert!(h.has("view-mode:Preview"));
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    h.keys("cmd-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Preview);
    assert!(h.read(|o, _| o.file_preview.is_some()));
    h.keys("cmd-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    h.keys("cmd-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
}

#[gpui::test]
async fn review_toggle_switches_the_main_column(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    h.click("obs-review-toggle");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    assert!(h.has("obs-review-host"));
    h.click("obs-editor-toggle");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
}

fn many_files() -> Vec<String> {
    let mut files = vec!["src/main.go".to_string()];
    for i in 0..12 {
        files.push(format!("src/long-file-name-{i:02}.go"));
    }
    files
}

fn many_files_opts(files: Vec<String>, main: String) -> MockOptions {
    let (f1, f2, f3) = (files.clone(), files.clone(), files);
    MockOptions::default()
        .resolver("ListTree", move |a, _, _| {
            Reply::value(match s(a, 2).as_str() {
                "" => json!([{"name": "src", "path": "src", "dir": true}]),
                "src" => json!(
                    f1.iter()
                        .map(|p| json!({"name": &p[4..], "path": p, "dir": false}))
                        .collect::<Vec<_>>()
                ),
                _ => json!([]),
            })
        })
        .resolver("ListChangedFiles", move |_, _, _| Reply::value(json!(f2)))
        .resolver("GitStatus", move |_, _, _| {
            Reply::value(json!(
                f3.iter()
                    .map(|p| (p.clone(), json!("m")))
                    .collect::<serde_json::Map<_, _>>()
            ))
        })
        .resolver("ReadFile", move |a, _, _| {
            let path = s(a, 2);
            let body = if path == "src/main.go" {
                main.clone()
            } else {
                "package p\n".into()
            };
            Reply::value(file(&path, "go", &body, true))
        })
}

#[gpui::test]
async fn switching_back_to_a_workspace_keeps_file_tab_and_editor_scroll(cx: &mut TestAppContext) {
    let files = many_files();
    let mut body = vec![
        "package main".to_string(),
        String::new(),
        "func main() {".into(),
    ];
    for i in 0..120 {
        body.push(format!("\tprintln(\"line {i}\")"));
    }
    body.push("}".into());
    body.push(String::new());
    let mut h = mount(
        cx,
        many_files_opts(files.clone(), body.join("\n")),
        |_| {},
        |_| {},
    );
    open_main_go(&mut h);
    h.click("view-mode:Code");
    for f in &files[1..] {
        h.click(&format!("obs-node:file:{f}"));
    }
    // The chip is scrolled out of the strip; Playwright scrolls it in and clicks, which is
    // the chip's own handler.
    h.update(|o, window, cx| {
        o.open_path(crate::OpenRequest::new("src/main.go"), window, cx)
            .detach()
    });
    assert!(h.active_tab("src/main.go"));
    if h.read(|o, _| o.view_mode) != EditorViewMode::Code {
        h.click("view-mode:Code");
    }
    // The strip overflows; scroll it, and the editor, by hand.
    let max = h.read(|o, _| f32::from(o.file_tab_scroll.max_offset().x));
    assert!(max > 60., "strip overflow {max}");
    let tab_scroll = 120f32.min(max.max(40.));
    h.update(move |o, _, _| {
        o.file_tab_scroll
            .set_offset(gpui::point(gpui::px(-tab_scroll), gpui::px(0.)))
    });
    h.update(|o, _, cx| {
        if let Some(e) = o.main_editor.clone() {
            e.update(cx, |e, cx| e.set_scroll(600., 0., cx));
        }
    });
    let editor_scroll = h.read(|o, cx| o.main_editor.as_ref().map(|e| e.read(cx).scroll_top()));
    assert!(editor_scroll.is_some_and(|t| t > 80.));

    h.switch_to(PROJECT, "LOOP-2229");
    h.switch_to(PROJECT, WS);
    assert!(h.active_tab("src/main.go"));
    let left = h.read(|o, _| -f32::from(o.file_tab_scroll.offset().x));
    assert_eq!(left, tab_scroll);
    let top = h.read(|o, cx| o.main_editor.as_ref().map(|e| e.read(cx).scroll_top()));
    assert!((top.unwrap_or(0.) - editor_scroll.unwrap_or(0.)).abs() < 64.);

    h.click("obs-node:file:src/long-file-name-11.go");
    assert!(h.active_tab("src/long-file-name-11.go"));
    assert!(h.active_chip_visible_width() > 24.);
}

#[gpui::test]
async fn opening_an_off_screen_file_tab_scrolls_it_into_view(cx: &mut TestAppContext) {
    let files = many_files();
    let mut h = mount(
        cx,
        many_files_opts(files.clone(), "package main\n".into()),
        |_| {},
        |_| {},
    );
    open_main_go(&mut h);
    for f in &files[1..] {
        h.click(&format!("obs-node:file:{f}"));
    }
    assert!(h.active_tab("src/long-file-name-11.go"));
    let max = h.read(|o, _| f32::from(o.file_tab_scroll.max_offset().x));
    assert!(max > 60.);
    let strip = h.bounds("obs-tb-center").expect("strip");
    let main = h
        .bounds("tab-chip:file-tab:src/main.go")
        .expect("main chip");
    let seen = main.right().min(strip.right()) - main.left().max(strip.left());
    assert!(
        f32::from(seen) < 8.,
        "main.go chip still visible ({seen:?})"
    );
    open_main_go(&mut h);
    assert!(h.active_tab("src/main.go"));
    assert!(h.active_chip_visible_width() > 24.);
}

fn hunk(mid: &str) -> String {
    let mut v = vec!["package main".to_string(), String::new()];
    v.extend((0..20).map(|i| format!("const a{i} = {i}")));
    v.push(format!("func main() {{ println(\"{mid}\") }}"));
    v.extend((0..20).map(|i| format!("const b{i} = {i}")));
    v.push(String::new());
    v.join("\n")
}

fn diff_opts(current: std::sync::Arc<parking_lot::Mutex<String>>) -> MockOptions {
    let base = hunk("old");
    MockOptions::default()
        .resolver("ListChangedFiles", |_, _, _| {
            Reply::value(json!(["src/main.go"]))
        })
        .resolver("GitStatus", |_, _, _| {
            Reply::value(json!({"src/main.go": "m"}))
        })
        .resolver("WorkingStatus", |_, _, _| {
            Reply::value(json!({"src/main.go": "m"}))
        })
        .resolver("FileAtBase", move |_, _, _| Reply::value(base.clone()))
        .resolver("ReadFile", move |a, _, _| {
            Reply::value(file(&s(a, 2), "go", &current.lock(), true))
        })
}

fn merge_view(h: &mut H) -> (String, usize) {
    h.read(|o, cx| {
        o.merge_editor
            .as_ref()
            .map(|(_, e)| {
                let st = e.read(cx).state();
                (
                    st.doc.to_string(),
                    st.merge.as_ref().map_or(0, |m| m.collapsed.len()),
                )
            })
            .unwrap_or_default()
    })
}

#[gpui::test]
async fn changed_filter_open_collapses_unchanged_lines_on_first_diff_paint(
    cx: &mut TestAppContext,
) {
    let current = std::sync::Arc::new(parking_lot::Mutex::new(hunk("new")));
    let mut h = mount(cx, diff_opts(current), |_| {}, |_| {});
    assert!(!h.read(|o, _| o.browse_all));
    open_main_go(&mut h);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    let (doc, collapsed) = merge_view(&mut h);
    assert!(doc.contains("println(\"new\")"));
    assert!(collapsed > 0, "no collapsed regions");
}

#[gpui::test]
async fn agent_edit_while_diff_is_open_recollapses_unchanged_lines(cx: &mut TestAppContext) {
    let current = std::sync::Arc::new(parking_lot::Mutex::new(hunk("new")));
    let mut h = mount(cx, diff_opts(current.clone()), |_| {}, |_| {});
    open_main_go(&mut h);
    assert!(merge_view(&mut h).1 > 0);
    *current.lock() = hunk("newer");
    h.fs_changed();
    h.wait(100);
    let (doc, collapsed) = merge_view(&mut h);
    assert!(doc.contains("println(\"newer\")"));
    assert!(collapsed > 0);
}

fn artifact_assets(bodies: Vec<(&'static str, &'static str)>) -> MockOptions {
    MockOptions::default().resolver("FetchAsset", move |a, _, _| {
        use base64::Engine as _;
        let url = s(a, 0);
        match bodies.iter().find(|(p, _)| url.ends_with(p)) {
            Some((_, b)) => Reply::value(json!({
                "contentType": "text/markdown; charset=utf-8",
                "body": base64::engine::general_purpose::STANDARD.encode(b),
            })),
            None => Reply::value(json!({"status": 404, "message": "not found"})),
        }
    })
}

#[gpui::test]
async fn artifacts_pane_badges_unseen_creates_and_present_artifact_opens_it(
    cx: &mut TestAppContext,
) {
    let opts = artifact_assets(vec![(
        "art-unseen-1.md",
        "# Badge test\n\nPresented body.\n",
    )]);
    let mut h = mount(cx, opts, |_| {}, |_| {});
    assert!(h.has("panel-toggle:artifacts"));
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        0
    );
    let artifact = json!({
        "id": "art-unseen-1", "title": "Badge test", "kind": "md",
        "path": format!("workspaces/{WS}/artifacts/art-unseen-1.md"),
    });
    h.event(
        "artifact.created",
        json!({"workspaceId": WS, "artifact": artifact}),
    );
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
    assert_eq!(
        h.read(|o, cx| o.pane_badge_title(CodePanelKey::Artifacts, cx)),
        "Artifacts (1 new)"
    );
    // A workspaces store refresh re-passes the same id: the badge stays.
    h.event(
        "workspace.updated",
        json!({"workspaceId": WS, "phase": "working", "iteration": 1}),
    );
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
    h.event(
        "artifact.presented",
        json!({"workspaceId": WS, "artifact": artifact}),
    );
    h.wait(100);
    assert_eq!(h.read(|o, _| o.right_kind), CodePanelKey::Artifacts);
    assert!(h.read(|o, _| o.show_side));
    assert!(h.has("art-panel"));
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        0
    );
    assert!(h.active_tab("scratch://artifact-art-unseen-1.md"));
    assert_eq!(
        h.read(|o, _| o.scratch_tab_label("scratch://artifact-art-unseen-1.md")),
        "Badge test"
    );
}

#[gpui::test]
async fn artifact_full_screen_fills_the_window_and_esc_restores_it(cx: &mut TestAppContext) {
    let opts = artifact_assets(vec![("art-full-1.md", "# Full screen\n")]);
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_main_go(&mut h);
    assert!(h.has("crumb:reload"));
    assert!(!h.has("crumb:fullscreen"));
    let artifact = json!({
        "id": "art-full-1", "title": "Full screen", "kind": "md",
        "path": format!("workspaces/{WS}/artifacts/art-full-1.md"),
    });
    h.event(
        "artifact.presented",
        json!({"workspaceId": WS, "artifact": artifact}),
    );
    h.wait(100);
    assert!(h.active_tab("scratch://artifact-art-full-1.md"));
    h.click("view-mode:Code");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    h.click("crumb:fullscreen");
    assert!(h.read(|o, _| o.editor_fullscreen));
    // Full screen previews.
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Preview);
    assert!(!h.has("obs-crumbbar"));
    assert!(!h.has("obs-statusbar"));
    h.click("fs-exit");
    assert!(!h.read(|o, _| o.editor_fullscreen));
    h.click("crumb:fullscreen");
    assert!(h.read(|o, _| o.editor_fullscreen));
    h.keys("escape");
    assert!(!h.read(|o, _| o.editor_fullscreen));
    assert!(h.active_tab("scratch://artifact-art-full-1.md"));
}

fn artifact(id: &str, title: &str) -> Value {
    json!({
        "id": id, "title": title, "kind": "md",
        "path": format!("workspaces/{WS}/artifacts/{id}.md"),
    })
}

fn artifact_body(
    body: std::sync::Arc<parking_lot::Mutex<String>>,
) -> impl Fn(&[Value], &mut recur_services::mock::Seed, &recur_services::mock::Table) -> Reply
+ Send
+ Sync
+ 'static {
    move |a, _, _| {
        use base64::Engine as _;
        if !s(a, 0).contains("/artifacts/") {
            return Reply::value(json!({"status": 404, "message": "not found"}));
        }
        Reply::value(json!({
            "contentType": "text/markdown; charset=utf-8",
            "body": base64::engine::general_purpose::STANDARD.encode(body.lock().as_bytes()),
        }))
    }
}

/// The scratch doc behind an artifact tab.
fn scratch_text(h: &mut H, path: &str) -> String {
    let p = path.to_string();
    h.read(move |o, _| {
        o.scratch_docs
            .get(&p)
            .map(|d| d.content.clone())
            .unwrap_or_default()
    })
}

#[gpui::test]
async fn clicking_an_artifact_leaves_review_and_opens_it_in_the_editor(cx: &mut TestAppContext) {
    let art = artifact("art-from-review-1", "From review");
    let body = std::sync::Arc::new(parking_lot::Mutex::new("# From review\n".to_string()));
    let a2 = art.clone();
    let opts = MockOptions::default()
        .resolver("ListArtifacts", move |_, _, _| Reply::value(json!([a2])))
        .resolver("FetchAsset", artifact_body(body));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("panel-toggle:artifacts");
    assert!(h.has("art-row:art-from-review-1"));
    h.click("obs-review-toggle");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    assert!(!h.has("obs-editors"));
    h.click("art-row:art-from-review-1");
    h.wait(100);
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
    assert!(h.has("obs-editors"));
    assert!(h.active_tab("scratch://artifact-art-from-review-1.md"));
}

#[gpui::test]
async fn refocusing_the_editor_on_an_artifact_tab_does_not_outline_it_from_disk(
    cx: &mut TestAppContext,
) {
    let outlines = Calls::default();
    let o2 = outlines.clone();
    let body = std::sync::Arc::new(parking_lot::Mutex::new("# No outline\n".to_string()));
    let opts = MockOptions::default()
        .resolver("Outline", move |a, _, _| {
            if !s(a, 2).starts_with("scratch://") {
                return Reply::value(json!([]));
            }
            o2.lock().push(a.to_vec());
            Reply::error("no such file")
        })
        .resolver("FetchAsset", artifact_body(body));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.event(
        "artifact.presented",
        json!({"workspaceId": WS, "artifact": artifact("art-outline-1", "No outline")}),
    );
    h.wait(100);
    assert!(h.active_tab("scratch://artifact-art-outline-1.md"));
    h.keys("cmd-1");
    h.keys("cmd-2");
    let in_main =
        h.cx.update(|window, cx| h.obs.read(cx).main_focus.contains_focused(window, cx));
    assert!(in_main);
    assert!(outlines.lock().is_empty());
}

#[gpui::test]
async fn reload_on_an_artifact_tab_refetches_it_from_the_artifact_store(cx: &mut TestAppContext) {
    let art = artifact("art-reload-1", "Reload me");
    let body = std::sync::Arc::new(parking_lot::Mutex::new("# Version one\n".to_string()));
    let a2 = art.clone();
    let opts = MockOptions::default()
        .resolver("ListArtifacts", move |_, _, _| Reply::value(json!([a2])))
        .resolver("FetchAsset", artifact_body(body.clone()));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.event(
        "artifact.presented",
        json!({"workspaceId": WS, "artifact": art}),
    );
    h.wait(100);
    let path = "scratch://artifact-art-reload-1.md";
    assert!(scratch_text(&mut h, path).contains("Version one"));
    *body.lock() = "# Version two\n".into();
    h.click("crumb:reload");
    h.wait(100);
    let t = scratch_text(&mut h, path);
    assert!(
        t.contains("Version two") && !t.contains("Version one"),
        "{t:?}"
    );
}

#[gpui::test]
async fn artifact_rename_via_context_menu_and_hover_trash_delete(cx: &mut TestAppContext) {
    let art = std::sync::Arc::new(parking_lot::Mutex::new(artifact(
        "art-rename-1",
        "Old name",
    )));
    let deleted = std::sync::Arc::new(parking_lot::Mutex::new(false));
    let (a1, a2, d1, d2) = (art.clone(), art.clone(), deleted.clone(), deleted.clone());
    let body = std::sync::Arc::new(parking_lot::Mutex::new("# Body\n".to_string()));
    let opts = MockOptions::default()
        .resolver("ListArtifacts", move |_, _, _| {
            Reply::value(if *d1.lock() {
                json!([])
            } else {
                json!([*a1.lock()])
            })
        })
        .resolver("RenameArtifact", move |a, _, _| {
            let mut v = a2.lock();
            v["title"] = json!(s(a, 2));
            Reply::value(v.clone())
        })
        .resolver("DeleteArtifact", move |_, _, _| {
            *d2.lock() = true;
            Reply::null()
        })
        .resolver("FetchAsset", artifact_body(body));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    let a = art.lock().clone();
    h.event(
        "artifact.presented",
        json!({"workspaceId": WS, "artifact": a}),
    );
    h.wait(100);
    let path = "scratch://artifact-art-rename-1.md";
    assert_eq!(h.read(|o, _| o.scratch_tab_label(path)), "Old name");
    h.right_click("art-row:art-rename-1");
    assert!(h.has("art-ctx-rename") && h.has("art-ctx-reveal") && h.has("art-ctx-delete"));
    h.click("art-ctx-rename");
    h.keys("cmd-a");
    h.type_text("New name");
    h.keys("enter");
    h.wait(100);
    assert_eq!(h.read(|o, _| o.scratch_tab_label(path)), "New name");
    h.hover("art-row:art-rename-1");
    h.click("art-del:art-rename-1");
    h.click("art-del:art-rename-1");
    h.wait(100);
    assert!(!h.has("art-row:art-rename-1"));
    assert!(!h.has_tab(path));
}

#[gpui::test]
async fn artifacts_badge_increments_while_the_observatory_is_off_screen(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    h.click("panel-toggle:artifacts");
    assert_eq!(h.read(|o, _| o.right_kind), CodePanelKey::Artifacts);
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        0
    );
    // Briefing: the detail view is not on screen.
    h.state_update(|s, cx| {
        s.view = recur_view_core::ViewName::Briefing;
        cx.notify();
    });
    h.event(
        "artifact.created",
        json!({"workspaceId": WS, "artifact": artifact("art-inbox-1", "Off-screen")}),
    );
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
}

#[gpui::test]
async fn present_artifact_on_a_background_workspace_badges_that_workspace(cx: &mut TestAppContext) {
    let mut h = mount(cx, MockOptions::default(), |_| {}, |_| {});
    let other = "LOOP-2229";
    let art = json!({
        "id": "art-bg-1", "title": "Background", "kind": "md",
        "path": format!("workspaces/{other}/artifacts/art-bg-1.md"),
    });
    h.event(
        "artifact.created",
        json!({"workspaceId": other, "artifact": art}),
    );
    h.event(
        "artifact.presented",
        json!({"workspaceId": other, "artifact": art}),
    );
    h.switch_to(PROJECT, other);
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
}

fn two_tabs_opts(main: &'static str, other: &'static str) -> MockOptions {
    MockOptions::default()
        .resolver("ListTree", |a, _, _| {
            Reply::value(match s(a, 2).as_str() {
                "" => json!([{"name": "src", "path": "src", "dir": true}]),
                "src" => json!([
                    {"name": "main.go", "path": "src/main.go", "dir": false},
                    {"name": "other.go", "path": "src/other.go", "dir": false},
                ]),
                _ => json!([]),
            })
        })
        .resolver("ListChangedFiles", |_, _, _| {
            Reply::value(json!(["src/main.go", "src/other.go"]))
        })
        .resolver("GitStatus", |_, _, _| {
            Reply::value(json!({"src/main.go": "m", "src/other.go": "m"}))
        })
        .resolver("ReadFile", move |a, _, _| {
            let p = s(a, 2);
            Reply::value(file(
                &p,
                "go",
                if p.ends_with("other.go") { other } else { main },
                true,
            ))
        })
}

fn open_two(h: &mut H) {
    for name in ["main", "other"] {
        h.click(&format!("obs-node:file:src/{name}.go"));
        if h.has("view-mode:Code") {
            h.click("view-mode:Code");
        }
        assert!(h.editor_doc().contains(&format!("package {name}")));
    }
    assert_eq!(h.open_files().len(), 2);
}

#[gpui::test]
async fn dragging_a_file_tab_reorders_the_strip_and_survives_reload(cx: &mut TestAppContext) {
    let sessions = std::sync::Arc::new(parking_lot::Mutex::new(Value::Null));
    let (s1, s2) = (sessions.clone(), sessions.clone());
    let opts = two_tabs_opts("package main\n\nfunc main() {}\n", "package other\n")
        .resolver("GetEditorSession", move |_, _, _| {
            Reply::value(s1.lock().clone())
        })
        .resolver("UpdateEditorSession", move |a, _, _| {
            if let Some(meta) = a
                .get(1)
                .and_then(|p| p.get("meta"))
                .filter(|m| !m.is_null())
            {
                *s2.lock() = meta.clone();
            }
            Reply::null()
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_two(&mut h);
    assert_eq!(h.open_files(), vec!["src/main.go", "src/other.go"]);
    let first = h
        .bounds("tab-chip:file-tab:src/main.go")
        .expect("first chip");
    let last = h
        .bounds("tab-chip:file-tab:src/other.go:on")
        .expect("last chip");
    h.drag_points(
        first.center(),
        gpui::point(last.right() - gpui::px(4.), last.center().y),
    );
    assert_eq!(h.open_files(), vec!["src/other.go", "src/main.go"]);
    h.wait(1000);
    // A cold remount restores the order from the editor session.
    let stored = sessions.lock().clone();
    assert_eq!(stored["openFiles"], json!(["src/other.go", "src/main.go"]));
}

#[gpui::test]
async fn file_tab_context_menu_closes_others_and_all(cx: &mut TestAppContext) {
    let mut h = mount(
        cx,
        two_tabs_opts("package main\n", "package other\n"),
        |_| {},
        |_| {},
    );
    open_two(&mut h);
    h.right_click("tab-chip:file-tab:src/main.go");
    assert!(h.has("ctx:close") && h.has("ctx:close-others") && h.has("ctx:close-all"));
    h.click("ctx:close-others");
    assert_eq!(h.open_files(), vec!["src/main.go"]);
    assert!(h.active_tab("src/main.go"));
    assert!(h.editor_doc().contains("package main"));
    h.right_click("tab-chip:file-tab:src/main.go:on");
    // Close others is disabled with one tab: a click does nothing.
    h.click("ctx:close-others");
    assert_eq!(h.open_files(), vec!["src/main.go"]);
    h.right_click("tab-chip:file-tab:src/main.go:on");
    h.click("ctx:close-all");
    assert!(h.open_files().is_empty());
}

#[gpui::test]
async fn file_tab_close_all_asks_once_for_dirty_tabs_and_cancel_keeps_them(
    cx: &mut TestAppContext,
) {
    let mut h = mount(
        cx,
        two_tabs_opts("package main\n", "package other\n"),
        |_| {},
        |_| {},
    );
    open_two(&mut h);
    h.focus_editor();
    h.keys("cmd-end");
    h.type_text(" // edit");
    h.update(|o, window, cx| {
        o.open_path(crate::OpenRequest::new("src/main.go"), window, cx)
            .detach()
    });
    assert!(h.editor_doc().contains("package main"));
    h.focus_editor();
    h.keys("cmd-end");
    h.type_text(" // edit");
    h.wait(300);
    assert!(h.read(|o, _| o.tab_is_dirty("src/main.go") && o.tab_is_dirty("src/other.go")));
    h.right_click("tab-chip:file-tab:src/main.go:on");
    h.click("ctx:close-all");
    let prompt = h.cx.pending_prompt().expect("a discard prompt");
    assert!(
        prompt.0.contains("Discard unsaved edits in 2 tabs?")
            || prompt.1.contains("Discard unsaved edits in 2 tabs?"),
        "{prompt:?}"
    );
    h.cx.simulate_prompt_answer("Cancel");
    h.settle();
    assert_eq!(h.open_files().len(), 2);
    assert!(h.read(|o, _| o.tab_is_dirty("src/main.go") && o.tab_is_dirty("src/other.go")));
}
