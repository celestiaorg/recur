//! The Observatory. The project-setup and inbox / Briefing scenarios belong to the
//! views that host them; their Observatory halves (occluder, badges) are here. The review
//! surface's own scenarios live in `recur_view_review::spec::observatory`.

use gpui::TestAppContext;
use recur_domain::{Artifact, FileContent, TreeEntry};
use recur_editor::view_state::EditorViewMode;
use recur_services::AppEvent;
use recur_services::api::{Asset, AssetError};
use recur_services::events::{ArtifactEvent, WorkspaceUpdated};
use recur_services::fake::{Overrides, Seed};
use recur_view_core::prefs::CodePanelKey;

use super::harness::*;
use crate::model::MainSurface;

/// A tree of one untracked root file, changed versus base.
fn one_file(name: &'static str) -> Overrides {
    Overrides::default()
        .list_tree(move |_, (_, _, path)| {
            Ok(if path.is_empty() {
                tree(&[(name, name, false)])
            } else {
                Vec::new()
            })
        })
        .list_changed_files(move |_, _| Ok(strings(&[name])))
        .git_status(move |_, _| Ok(statuses(&[(name, "u")])))
        .working_status(move |_, _| Ok(statuses(&[(name, "u")])))
}

/// The fake's tree: `src/main.go`, changed (`m`) versus base.
fn open_main_go(h: &mut H) {
    h.click("obs-node:file:src/main.go");
}

#[gpui::test]
async fn loop_detail_mounts_the_observatory(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    assert!(h.has("obs"));
    assert!(h.has("obs-left"));
    assert!(h.has("agent-chat"));
    // No unread on the Agent toggle.
    let n = h.read(|o, cx| o.pane_badge_count(CodePanelKey::Maestro, cx));
    assert_eq!(n, 0);
}

#[gpui::test]
async fn agent_chat_icon_shows_an_unread_badge_from_workspace_attention(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
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
    h.fake.simulate_chat_turn_idle(WS, "MA");
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
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
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
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
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
    let w = writes.clone();
    let opts = Overrides::default().write_file(move |_, (p, ws, path, content)| {
        push(&w, &[&p, &ws, &path, &content]);
        Ok(())
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_main_go(&mut h);
    h.click("view-mode:Code");
    h.focus_editor();
    h.keys("secondary-end");
    h.type_text(" // edit");
    h.wait(300);
    assert!(h.read(|o, _| o.tab_is_dirty("src/main.go")));
    h.keys("secondary-s");
    h.wait(300);
    assert!(h.active_tab("src/main.go"));
    assert!(!h.read(|o, _| o.tab_is_dirty("src/main.go")));
    let w = writes.lock().clone();
    assert_eq!(w.len(), 1);
    assert!(w[0][3].contains("}\n // edit"), "wrote {:?}", w[0]);
}

/// willSave runs the mocked gofmt transform before WriteFile: the Go bundle's
/// save command reformats the buffer (the mock collapses blank-line runs and trims trailing
/// blanks) before it is written.
#[gpui::test]
async fn will_save_runs_the_gofmt_transform_before_write_file(cx: &mut TestAppContext) {
    let writes = Calls::default();
    let w = writes.clone();
    let opts = Overrides::default()
        .read_file(|_, _| {
            Ok(FileContent {
                size: 40,
                ..file(
                    "src/main.go",
                    "go",
                    "package main\n\n\nfunc main() {}   \n",
                    true,
                )
            })
        })
        .write_file(move |_, (p, ws, path, content)| {
            push(&w, &[&p, &ws, &path, &content]);
            Ok(())
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    open_main_go(&mut h);
    h.click("view-mode:Code");
    h.focus_editor();
    // Touch the buffer so Save has something to write.
    h.type_text(" ");
    h.keys("backspace");
    h.wait(100);
    h.keys("secondary-s");
    h.wait(500);
    let w = writes.lock().clone();
    assert!(!w.is_empty());
    let text = w[0][3].clone();
    assert!(
        !regex::Regex::new(r"[ \t]+\n").unwrap().is_match(&text),
        "{text:?}"
    );
    assert!(!text.contains("\n\n\n"), "{text:?}");
}

#[gpui::test]
async fn opening_a_folder_keeps_the_open_file_and_breadcrumbs(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
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
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
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

fn notes_tree(
    _: &mut Seed,
    (_, _, path): (String, String, String),
) -> recur_services::Result<Vec<TreeEntry>> {
    Ok(match path.as_str() {
        "" => tree(&[("notes.txt", "notes.txt", false), ("src", "src", true)]),
        "src" => tree(&[("main.go", "src/main.go", false)]),
        _ => Vec::new(),
    })
}

#[gpui::test]
async fn dragging_an_explorer_row_onto_a_folder_moves_it_there(cx: &mut TestAppContext) {
    let renames = Calls::default();
    let r2 = renames.clone();
    let opts = Overrides::default()
        .list_tree(notes_tree)
        .rename(move |_, (p, ws, from, to)| {
            push(&r2, &[&p, &ws, &from, &to]);
            Ok(())
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-scope:all");
    h.drag("obs-node:file:notes.txt", "obs-node:dir:src");
    let r: Vec<(String, String)> = renames
        .lock()
        .iter()
        .map(|a| (a[2].clone(), a[3].clone()))
        .collect();
    assert_eq!(r, vec![("notes.txt".into(), "src/notes.txt".into())]);
    // A folder onto a file's row: the row's folder is the root, where it already is.
    h.drag("obs-node:dir:src", "obs-node:file:notes.txt");
    assert_eq!(renames.lock().len(), 1);
}

#[gpui::test]
async fn double_click_rename_stays_focused_after_a_slow_all_scope_open(cx: &mut TestAppContext) {
    let opts = Overrides::default()
        .list_tree(notes_tree)
        .read_file(|_, (_, ws, path)| {
            let body = if path == "notes.txt" {
                "hello notes\n"
            } else {
                "package main\n\nfunc main() {}\n"
            };
            Ok(file(
                &path,
                if path.ends_with(".txt") { "text" } else { "go" },
                body,
                !ws.is_empty(),
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
    let opts = Overrides::default().list_changed_files(move |_, _| Ok(c2.lock().clone()));
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
    let opts = Overrides::default()
        .read_file(move |_, (_, ws, path)| Ok(file(&path, "go", &c2.lock(), !ws.is_empty())));
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
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    open_main_go(&mut h);
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
    h.keys("secondary-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    h.keys("secondary-shift-v");
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

fn png_asset(url: &str) -> Asset {
    let mime = if url.contains(".mp4") {
        "video/mp4"
    } else {
        "image/png"
    };
    Asset {
        content_type: mime.into(),
        cache_control: String::new(),
        body: PNG.to_vec(),
    }
}

/// A file the editor shows as media: no text, its size and mime.
fn media(path: &str, size: usize, writable: bool, mime: &str) -> FileContent {
    FileContent {
        size: size as i64,
        mime: mime.into(),
        ..file(path, "", "", writable)
    }
}

#[gpui::test]
async fn binary_images_skip_code_and_modified_ones_offer_preview_and_diff(cx: &mut TestAppContext) {
    let status = statuses(&[
        ("src/main.go", "m"),
        ("src/shot.png", "u"),
        ("src/icon.png", "m"),
        ("src/gone.png", "d"),
    ]);
    let (st1, st2) = (status.clone(), status.clone());
    let opts = Overrides::default()
        .list_tree(|_, (_, _, path)| {
            Ok(match path.as_str() {
                "" => tree(&[("src", "src", true)]),
                "src" => tree(&[
                    ("main.go", "src/main.go", false),
                    ("shot.png", "src/shot.png", false),
                    ("icon.png", "src/icon.png", false),
                    ("gone.png", "src/gone.png", false),
                ]),
                _ => Vec::new(),
            })
        })
        .list_changed_files(|_, _| {
            Ok(strings(&[
                "src/main.go",
                "src/shot.png",
                "src/icon.png",
                "src/gone.png",
            ]))
        })
        .git_status(move |_, _| Ok(st1.clone()))
        .working_status(move |_, _| Ok(st2.clone()))
        .read_file(|_, (_, ws, path)| {
            if path.ends_with(".png") {
                return Ok(media(&path, PNG.len(), !ws.is_empty(), "image/png"));
            }
            Ok(file(&path, "go", "package main\n\nfunc main() {}\n", true))
        })
        .fetch_asset(|_, url| Ok(png_asset(&url)));
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
async fn videos_play_in_the_pane_and_zoom_into_the_lightbox(cx: &mut TestAppContext) {
    let opened = Calls::default();
    let o2 = opened.clone();
    let opts = one_file("clip.mp4")
        .read_file(|_, (_, _, path)| Ok(media(&path, 2048, true, "video/mp4")))
        .open_in_editor(move |_, (p, ws, path, line)| {
            push(&o2, &[&p, &ws, &path, &line.to_string()]);
            Ok(())
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:clip.mp4");
    assert!(h.has("obs-video"));
    assert!(h.has("video"), "the pane mounts a player");
    assert!(!h.has("view-mode:Code"));
    // `zoomVideo`: the lightbox plays the clip; nothing opens outside the app.
    h.click("video-zoom");
    assert!(h.has("chat-att-lb"));
    assert!(!h.has("lb-copy"), "a video has no Copy image");
    assert!(opened.lock().is_empty());
    h.keys("escape");
    assert!(!h.has("chat-att-lb"));
    assert!(h.has("obs-video"));
}

#[gpui::test]
async fn cmd_shift_v_cycles_code_diff_and_preview(cx: &mut TestAppContext) {
    let opts = Overrides::default()
        .list_tree(|_, (_, _, path)| {
            Ok(match path.as_str() {
                "" => tree(&[("README.md", "README.md", false), ("src", "src", true)]),
                "src" => tree(&[("main.go", "src/main.go", false)]),
                _ => Vec::new(),
            })
        })
        .list_changed_files(|_, _| Ok(strings(&["src/main.go", "README.md"])))
        .git_status(|_, _| Ok(statuses(&[("src/main.go", "m"), ("README.md", "m")])))
        .working_status(|_, _| Ok(statuses(&[("src/main.go", "m"), ("README.md", "m")])))
        .read_file(|_, (_, _, path)| {
            Ok(if path.ends_with(".md") {
                file(&path, "markdown", "# Hello\n\nWorld\n", true)
            } else {
                file(&path, "go", "package main\n\nfunc main() {}\n", true)
            })
        })
        .file_at_base(|_, (_, _, path)| {
            Ok(if path.ends_with(".md") {
                "# Hello\n"
            } else {
                "package main\n\nfunc main() {}\n"
            }
            .into())
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:README.md");
    assert!(h.has("view-mode:Preview"));
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    h.keys("secondary-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Preview);
    assert!(h.read(|o, _| o.file_preview.is_some()));
    h.keys("secondary-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Code);
    h.keys("secondary-shift-v");
    assert_eq!(h.read(|o, _| o.view_mode), EditorViewMode::Diff);
    assert!(h.read(|o, _| o.merge_editor.is_some()));
}

#[gpui::test]
async fn review_toggle_switches_the_main_column(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
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

fn many_files_opts(files: Vec<String>, main: String) -> Overrides {
    let (f1, f2, f3) = (files.clone(), files.clone(), files);
    Overrides::default()
        .list_tree(move |_, (_, _, path)| {
            Ok(match path.as_str() {
                "" => tree(&[("src", "src", true)]),
                "src" => f1
                    .iter()
                    .map(|p| TreeEntry {
                        name: p[4..].into(),
                        path: p.clone(),
                        ..Default::default()
                    })
                    .collect(),
                _ => Vec::new(),
            })
        })
        .list_changed_files(move |_, _| Ok(f2.clone()))
        .git_status(move |_, _| Ok(f3.iter().map(|p| (p.clone(), "m".to_owned())).collect()))
        .read_file(move |_, (_, _, path)| {
            let body = if path == "src/main.go" {
                main.clone()
            } else {
                "package p\n".into()
            };
            Ok(file(&path, "go", &body, true))
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

fn diff_opts(current: std::sync::Arc<parking_lot::Mutex<String>>) -> Overrides {
    let base = hunk("old");
    Overrides::default()
        .list_changed_files(|_, _| Ok(strings(&["src/main.go"])))
        .git_status(|_, _| Ok(statuses(&[("src/main.go", "m")])))
        .working_status(|_, _| Ok(statuses(&[("src/main.go", "m")])))
        .file_at_base(move |_, _| Ok(base.clone()))
        .read_file(move |_, (_, _, path)| Ok(file(&path, "go", &current.lock(), true)))
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

fn markdown(body: &str) -> Asset {
    Asset {
        content_type: "text/markdown; charset=utf-8".into(),
        cache_control: String::new(),
        body: body.as_bytes().to_vec(),
    }
}

fn artifact_assets(bodies: Vec<(&'static str, &'static str)>) -> Overrides {
    Overrides::default().fetch_asset(move |_, url| {
        match bodies.iter().find(|(p, _)| url.ends_with(p)) {
            Some((_, b)) => Ok(markdown(b)),
            None => Err(AssetError::new(404, "not found")),
        }
    })
}

fn created(workspace_id: &str, artifact: Artifact) -> AppEvent {
    AppEvent::ArtifactCreated(ArtifactEvent {
        workspace_id: workspace_id.into(),
        artifact,
    })
}

fn presented(workspace_id: &str, artifact: Artifact) -> AppEvent {
    AppEvent::ArtifactPresented(ArtifactEvent {
        workspace_id: workspace_id.into(),
        artifact,
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
    let artifact = artifact("art-unseen-1", "Badge test");
    h.event(created(WS, artifact.clone()));
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
    assert_eq!(
        h.read(|o, cx| o.pane_badge_title(CodePanelKey::Artifacts, cx)),
        "Artifacts (1 new)"
    );
    // A workspaces store refresh re-passes the same id: the badge stays.
    h.event(AppEvent::WorkspaceUpdated(std::sync::Arc::new(
        WorkspaceUpdated {
            workspace_id: WS.into(),
            phase: Some("working".into()),
            iteration: Some(1),
            ..Default::default()
        },
    )));
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
    h.event(presented(WS, artifact));
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
    h.event(presented(WS, artifact("art-full-1", "Full screen")));
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
    let main = h.bounds("obs-main").unwrap();
    let vp = h.cx.update(|w, _| w.viewport_size());
    assert_eq!((main.origin, main.size), (gpui::Point::default(), vp));
    h.click("fs-exit");
    assert!(!h.read(|o, _| o.editor_fullscreen));
    h.click("crumb:fullscreen");
    assert!(h.read(|o, _| o.editor_fullscreen));
    h.keys("escape");
    assert!(!h.read(|o, _| o.editor_fullscreen));
    assert!(h.active_tab("scratch://artifact-art-full-1.md"));
}

fn artifact_in(workspace: &str, id: &str, title: &str) -> Artifact {
    Artifact {
        id: id.into(),
        title: title.into(),
        kind: "md".into(),
        path: format!("workspaces/{workspace}/artifacts/{id}.md"),
        ..Default::default()
    }
}

fn artifact(id: &str, title: &str) -> Artifact {
    artifact_in(WS, id, title)
}

fn artifact_body(
    body: std::sync::Arc<parking_lot::Mutex<String>>,
) -> impl Fn(&mut Seed, String) -> Result<Asset, AssetError> + Send + Sync + 'static {
    move |_, url| {
        if !url.contains("/artifacts/") {
            return Err(AssetError::new(404, "not found"));
        }
        Ok(markdown(&body.lock()))
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
    let opts = Overrides::default()
        .list_artifacts(move |_, _| Ok(vec![a2.clone()]))
        .fetch_asset(artifact_body(body));
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
    let opts = Overrides::default()
        .outline(move |_, (p, ws, path)| {
            if !path.starts_with("scratch://") {
                return Ok(Vec::new());
            }
            push(&o2, &[&p, &ws, &path]);
            Err(recur_services::Error::new("no such file"))
        })
        .fetch_asset(artifact_body(body));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.event(presented(WS, artifact("art-outline-1", "No outline")));
    h.wait(100);
    assert!(h.active_tab("scratch://artifact-art-outline-1.md"));
    h.keys("secondary-1");
    h.keys("secondary-2");
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
    let opts = Overrides::default()
        .list_artifacts(move |_, _| Ok(vec![a2.clone()]))
        .fetch_asset(artifact_body(body.clone()));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.event(presented(WS, art));
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
    let opts = Overrides::default()
        .list_artifacts(move |_, _| {
            Ok(if *d1.lock() {
                Vec::new()
            } else {
                vec![a1.lock().clone()]
            })
        })
        .rename_artifact(move |_, (_, _, title)| {
            let mut v = a2.lock();
            v.title = title;
            Ok(v.clone())
        })
        .delete_artifact(move |_, _| {
            *d2.lock() = true;
            Ok(())
        })
        .fetch_asset(artifact_body(body));
    let mut h = mount(cx, opts, |_| {}, |_| {});
    let a = art.lock().clone();
    h.event(presented(WS, a));
    h.wait(100);
    let path = "scratch://artifact-art-rename-1.md";
    assert_eq!(h.read(|o, _| o.scratch_tab_label(path)), "Old name");
    h.right_click("art-row:art-rename-1");
    assert!(h.has("art-ctx-rename") && h.has("art-ctx-reveal") && h.has("art-ctx-delete"));
    h.click("art-ctx-rename");
    h.keys("secondary-a");
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
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
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
    h.event(created(WS, artifact("art-inbox-1", "Off-screen")));
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
}

#[gpui::test]
async fn present_artifact_on_a_background_workspace_badges_that_workspace(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    let other = "LOOP-2229";
    let art = artifact_in(other, "art-bg-1", "Background");
    h.event(created(other, art.clone()));
    h.event(presented(other, art));
    h.switch_to(PROJECT, other);
    assert_eq!(
        h.read(|o, cx| o.pane_badge_count(CodePanelKey::Artifacts, cx)),
        1
    );
}

fn two_tabs_opts(main: &'static str, other: &'static str) -> Overrides {
    Overrides::default()
        .list_tree(|_, (_, _, path)| {
            Ok(match path.as_str() {
                "" => tree(&[("src", "src", true)]),
                "src" => tree(&[
                    ("main.go", "src/main.go", false),
                    ("other.go", "src/other.go", false),
                ]),
                _ => Vec::new(),
            })
        })
        .list_changed_files(|_, _| Ok(strings(&["src/main.go", "src/other.go"])))
        .git_status(|_, _| Ok(statuses(&[("src/main.go", "m"), ("src/other.go", "m")])))
        .read_file(move |_, (_, _, p)| {
            Ok(file(
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
    let sessions = std::sync::Arc::new(parking_lot::Mutex::new(None));
    let (s1, s2) = (sessions.clone(), sessions.clone());
    let opts = two_tabs_opts("package main\n\nfunc main() {}\n", "package other\n")
        .get_editor_session(move |_, _| Ok(s1.lock().clone()))
        .update_editor_session(move |_, (_, patch)| {
            if let Some(meta) = &patch.meta {
                *s2.lock() = Some(session_from_meta(meta, Default::default()));
            }
            Ok(())
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
    let stored = sessions.lock().clone().unwrap_or_default();
    assert_eq!(
        stored.open_files,
        Some(strings(&["src/other.go", "src/main.go"]))
    );
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
    h.keys("secondary-end");
    h.type_text(" // edit");
    h.update(|o, window, cx| {
        o.open_path(crate::OpenRequest::new("src/main.go"), window, cx)
            .detach()
    });
    assert!(h.editor_doc().contains("package main"));
    h.focus_editor();
    h.keys("secondary-end");
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

#[gpui::test]
async fn opt_cmd_r_toggles_review_from_the_editor_not_while_files_is_focused(
    cx: &mut TestAppContext,
) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    open_main_go(&mut h);
    assert!(h.active_tab("src/main.go"));
    // Diff's editable side has focus (it used to count as typing and swallow ⌥⌘R).
    h.update(|o, window, cx| {
        if let Some((_, e)) = o.merge_editor.clone() {
            let f = gpui_kit::Focusable::focus_handle(e.read(cx), cx);
            window.focus(&f, cx);
        }
    });
    h.keys("secondary-alt-r");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    assert!(h.has("obs-review-host"));
    h.keys("secondary-alt-r");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
    h.update(|o, window, cx| window.focus(&o.files_focus, cx));
    h.keys("secondary-alt-r");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
}

#[gpui::test]
async fn review_changes_is_remembered_on_hold_warm_remount_and_cold_reload(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let (a, b, c) = (WS, "LOOP-2229", "LOOP-2235");
    let store = SessionStore::default();
    let mut h = mount(cx, store.install(Overrides::default()), |_| {}, |_| {});
    h.click("obs-review-toggle");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    h.switch_to(PROJECT, b);
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
    h.switch_to(PROJECT, a);
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    h.switch_to(PROJECT, b);
    h.switch_to(PROJECT, c);
    h.switch_to(PROJECT, a);
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    assert!(h.has("obs-review-host"));
    h.wait(1000);
    h.quit();
    let mut h = mount(
        restarted,
        store.install(Overrides::default()),
        |_| {},
        |_| {},
    );
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Review);
    assert!(h.has("obs-review-host"));
}

#[gpui::test]
async fn pane_kinds_and_visibility_are_remembered_on_hold_warm_remount_and_cold_reload(
    cx: &mut TestAppContext,
    restarted: &mut TestAppContext,
) {
    let (a, b, c) = (WS, "LOOP-2229", "LOOP-2235");
    let store = SessionStore::default();
    let mut h = mount(cx, store.install(Overrides::default()), |_| {}, |_| {});
    h.click("panel-toggle:findings");
    h.click("panel-toggle:scm");
    assert_eq!(h.read(|o, _| o.left_kind), CodePanelKey::Findings);
    assert_eq!(h.read(|o, _| o.right_kind), CodePanelKey::Scm);
    h.click("obs-left-toggle");
    h.click("obs-side-toggle");
    assert!(!h.has("obs-left") && !h.has("obs-side"));

    h.switch_to(PROJECT, b);
    assert!(h.has("obs-left") && h.has("obs-side"));
    assert_eq!(h.read(|o, _| o.left_kind), CodePanelKey::Maestro);
    assert_eq!(h.read(|o, _| o.right_kind), CodePanelKey::Files);

    h.switch_to(PROJECT, a);
    assert!(!h.has("obs-left") && !h.has("obs-side"));
    h.switch_to(PROJECT, b);
    h.switch_to(PROJECT, c);
    h.switch_to(PROJECT, a);
    assert!(!h.has("obs-left") && !h.has("obs-side"));
    h.click("obs-left-toggle");
    h.click("obs-side-toggle");
    assert_eq!(h.read(|o, _| o.left_kind), CodePanelKey::Findings);
    assert_eq!(h.read(|o, _| o.right_kind), CodePanelKey::Scm);
    h.click("obs-left-toggle");
    h.click("obs-side-toggle");
    h.wait(1000);
    h.quit();

    let mut h = mount(
        restarted,
        store.install(Overrides::default()),
        |_| {},
        |_| {},
    );
    assert!(!h.has("obs-left") && !h.has("obs-side"));
    h.click("obs-left-toggle");
    h.click("obs-side-toggle");
    assert_eq!(h.read(|o, _| o.left_kind), CodePanelKey::Findings);
    assert_eq!(h.read(|o, _| o.right_kind), CodePanelKey::Scm);
}

#[gpui::test]
async fn a_hidden_review_leaves_the_layout(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    h.click("obs-review-toggle");
    assert!(h.has("obs-review-host"));
    h.click("obs-editor-toggle");
    assert_eq!(h.read(|o, cx| o.main_surface(cx)), MainSurface::Editor);
    // `.obs-review-host.off` is `display: none`; the surface itself stays for a warm return.
    assert!(!h.has("obs-review-host"));
    assert!(h.read(|o, _| o.review_surface.is_some()));
    h.click("obs-review-toggle");
    assert!(h.has("obs-review-host"));
}

/// A watcher event refreshes the working status; one that finds it unchanged must not notify
/// the Observatory, whose source control changes list (beside the commit message) re-renders on
/// every notify: on a large change set that turned keys in the commit box into 700ms frames.
#[gpui::test]
async fn a_working_status_refresh_that_changes_nothing_does_not_notify(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    h.click("panel-toggle:scm");
    h.update(|o, _, cx| o.refresh_working_status(cx).detach());
    let n = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = n.clone();
    let obs = h.obs.clone();
    let _sub =
        h.cx.update(|_, cx| cx.observe(&obs, move |_, _| seen.set(seen.get() + 1)));
    h.update(|o, _, cx| o.refresh_working_status(cx).detach());
    assert_eq!(n.get(), 0);
}

/// The other watcher refreshes (the changed files, the folders on screen) likewise notify
/// only on a change: each redrew the explorer tree beside the commit message.
#[gpui::test]
async fn a_live_refresh_that_changes_nothing_does_not_notify(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    h.update(|o, _, cx| {
        o.refresh_changed_files(cx).detach();
        o.refresh_loaded_dirs(cx).detach();
    });
    let n = std::rc::Rc::new(std::cell::Cell::new(0));
    let seen = n.clone();
    let obs = h.obs.clone();
    let _sub =
        h.cx.update(|_, cx| cx.observe(&obs, move |_, _| seen.set(seen.get() + 1)));
    h.update(|o, _, cx| {
        o.refresh_changed_files(cx).detach();
        o.refresh_loaded_dirs(cx).detach();
    });
    assert_eq!(n.get(), 0);
}

/// A file event while the review shows reloads the review alone: it reached the review as an
/// Observatory notify, which redrew every cached pane (the changes list) on each event, keys
/// typed meanwhile included.
#[gpui::test]
async fn a_file_event_under_the_review_does_not_notify_the_observatory(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    h.update(|o, _, cx| o.click_review_toggle(cx));
    h.settle();
    let before = h.read(|o, _| o.review_refresh);
    let notifies = std::rc::Rc::new(std::cell::Cell::new(0));
    let n = notifies.clone();
    let obs = h.obs.clone();
    let _sub =
        h.cx.update(|_, cx| cx.observe(&obs, move |_, _| n.set(n.get() + 1)));
    h.fs_changed();
    h.wait(crate::obs::REVIEW_REFRESH_MS + 200);
    assert_eq!(
        h.read(|o, _| o.review_refresh),
        before + 1,
        "the review did not reload"
    );
    assert_eq!(notifies.get(), 0, "a file event notified the Observatory");
}

/// A long changes list lays out the rows near its viewport: a status change on a large change
/// set redrew every row, and that frame landed on keys typed in the commit box beside it.
#[gpui::test]
async fn a_long_changes_list_lays_out_the_rows_near_its_viewport(cx: &mut TestAppContext) {
    let opts = Overrides::default().working_status(|_, _| {
        Ok((0..250)
            .map(|i| (format!("f{i:03}.txt"), "m".to_owned()))
            .collect())
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("panel-toggle:scm");
    h.update(|o, _, cx| o.refresh_working_status(cx).detach());
    h.settle();
    assert!(h.has("scm-stage:u:f000.txt"));
    assert!(!h.has("scm-stage:u:f249.txt"));
    let scroll = h.read(|o, _| o.scm_scroll.clone());
    let max = scroll.max_offset().y;
    assert!(
        max > gpui::px(4000.),
        "spacers keep the list's height: {max:?}"
    );
    scroll.set_offset(gpui::point(gpui::px(0.), -max));
    h.update(|_, _, cx| cx.notify());
    h.settle();
    assert!(h.has("scm-stage:u:f249.txt"));
    assert!(!h.has("scm-stage:u:f000.txt"));
}

#[gpui::test]
async fn pdf_find_opens_from_the_crumb_bar_and_takes_typing(cx: &mut TestAppContext) {
    const PDF: &[u8] = include_bytes!("../../../../ui/gallery/content/page.pdf");
    let opts = one_file("page.pdf")
        .read_file(|_, (_, _, path)| Ok(media(&path, PDF.len(), true, "application/pdf")))
        .fetch_asset(|_, _| {
            Ok(Asset {
                content_type: "application/pdf".into(),
                cache_control: String::new(),
                body: PDF.to_vec(),
            })
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-node:file:page.pdf");
    h.wait(500);
    assert!(h.read(|o, _| o.can_toggle_find()));
    h.click("crumb:find");
    let pdf = h.read(|o, _| o.pdf_preview.clone()).unwrap();
    assert!(h.read(|o, _| o.find_panel_open));
    h.keys("t h e");
    let label = h.cx.update(|_, cx| pdf.read(cx).find_count_label(cx));
    assert!(label.starts_with("1 / "), "{label:?}");
    h.keys("escape");
    assert!(h.cx.update(|_, cx| !pdf.read(cx).is_search_open()));
    assert!(!h.read(|o, _| o.find_panel_open));
}

#[gpui::test]
async fn a_tree_menu_opened_near_the_window_bottom_stays_at_the_pointer(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    let vh = h.update(|_, w, _| f32::from(w.viewport_size().height));
    let y = vh - 40.;
    h.update(|o, _, cx| o.open_ctx(40., y, "src/main.go", false, "main.go", false, cx));
    let b = h.bounds("obs-ctx").expect("menu open");
    assert!(f32::from(b.bottom()) <= vh, "{b:?} overflows {vh}");
    assert!(
        (f32::from(b.bottom()) - y).abs() < 2.,
        "{b:?} not at the pointer {y}"
    );
}

#[gpui::test]
async fn the_blank_space_menu_ends_without_a_separator(cx: &mut TestAppContext) {
    let mut h = mount(cx, Overrides::default(), |_| {}, |_| {});
    h.update(|o, _, cx| o.open_ctx(40., 40., "", true, "", false, cx));
    let menu = h.bounds("obs-ctx").expect("menu open");
    let last = h.bounds("ctx:new-folder").expect("new folder row");
    assert!(
        f32::from(menu.bottom() - last.bottom()) < 6.,
        "{menu:?} {last:?}"
    );
}
