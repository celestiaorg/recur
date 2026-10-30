//! Explorer scope: the Changed view built from the changed-path list (no directory
//! listings), sticky folders, the remembered scope, worktree switches without stale rows or
//! a flash of skeleton, changed submodule directories, and muted gitignored entries.

use std::sync::Arc;

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_domain::TreeEntry;
use recur_services::fake::{Overrides, hold};
use recur_services::{Error, Result};
use recur_ui::widgets::ToastKind;

use super::harness::*;

fn changed() -> Vec<String> {
    let mut v = Vec::new();
    for d in 0..80 {
        for f in 0..5 {
            v.push(format!("internal/pkg{}/sub{d}/file{f}.go", d % 20));
        }
    }
    v
}

fn children_of(list: &[String], dir: &str) -> Vec<TreeEntry> {
    let mut kids: Vec<(String, bool)> = Vec::new();
    for p in list {
        let rest = if dir.is_empty() {
            p.as_str()
        } else if let Some(r) = p.strip_prefix(&format!("{dir}/")) {
            r
        } else {
            continue;
        };
        let mut parts = rest.split('/');
        let name = parts.next().unwrap_or("").to_string();
        let is_dir = parts.next().is_some();
        if !kids.iter().any(|(n, _)| *n == name) {
            kids.push((name, is_dir));
        }
    }
    kids.into_iter()
        .map(|(n, d)| TreeEntry {
            path: if dir.is_empty() { n.clone() } else { format!("{dir}/{n}") },
            name: n,
            dir: d,
            ..Default::default()
        })
        .collect()
}

fn status_of(list: &[String]) -> std::collections::BTreeMap<String, String> {
    list.iter().map(|p| (p.clone(), "m".to_owned())).collect()
}

fn changed_opts(list: Vec<String>) -> Overrides {
    let (l1, l2, l3) = (list.clone(), list.clone(), list);
    Overrides::default()
        .list_changed_files(move |_, _| Ok(l1.clone()))
        .git_status(move |_, _| Ok(status_of(&l2)))
        .working_status(move |_, _| Ok(status_of(&l3)))
}

#[gpui::test]
async fn the_changed_scope_renders_without_listing_directories(cx: &mut TestAppContext) {
    let list = changed();
    let listed: Arc<Mutex<Vec<String>>> = Arc::default();
    let (l1, l2, c1, c2) = (listed.clone(), listed.clone(), list.clone(), list.clone());
    let opts = changed_opts(list.clone())
        .list_tree(move |_, (_, _, path)| {
            let kids = children_of(&c1, &path);
            l1.lock().push(path);
            Ok(kids)
        })
        .list_trees(move |_, (_, _, dirs)| {
            l2.lock().extend(dirs.clone());
            Ok(dirs
                .iter()
                .map(|d| (d.clone(), children_of(&c2, d)))
                .collect())
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.wait(1500);
    // Every changed file and its folders are rows…
    let rows = h.read(|o, cx| o.explorer_row_count(cx));
    assert!(rows > list.len(), "{rows}");
    // …and the only directory asked about is the root.
    assert!(
        listed.lock().iter().all(String::is_empty),
        "{:?}",
        listed.lock()
    );
}

#[gpui::test]
async fn explorer_folders_stick_while_scrolling(cx: &mut TestAppContext) {
    let mut h = mount(cx, changed_opts(changed()), |_| {}, |_| {});
    h.wait(200);
    let folder = "obs-node:dir:internal";
    assert!(h.has(folder));
    // Collapse: the folder is a plain row; expand again.
    h.click(folder);
    assert!(
        !h.read(|o, _| o.changed_collapsed.is_empty())
            || !h.read(|o, _| o.expanded.contains("internal"))
    );
    h.click(folder);
    h.update(|o, _, _| {
        o.files_scroll
            .set_offset(gpui::point(gpui::px(0.), gpui::px(-240.)))
    });
    h.settle();
    let pane = h.bounds("obs-files").expect("files");
    let stuck = h.bounds("obs-stuck:internal").expect("stuck internal");
    assert!(f32::from((stuck.top() - pane.top()).abs()) < 2.);
    let mut dirs: Vec<String> = vec!["internal".into()];
    for p in changed() {
        let parts: Vec<&str> = p.split('/').collect();
        for n in 2..parts.len() {
            let d = parts[..n].join("/");
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
    }
    h.settle();
    let fades = dirs
        .iter()
        .filter(|p| {
            h.cx.debug_bounds(Box::leak(format!("obs-node-fade:{p}").into_boxed_str()))
                .is_some()
        })
        .count();
    assert_eq!(fades, 1);
}

#[gpui::test]
async fn an_expanded_folder_does_not_hide_the_next_folder_name(cx: &mut TestAppContext) {
    let list = vec!["alpha/a.go".to_string(), "beta/b.go".to_string()];
    let mut h = mount(cx, changed_opts(list), |_| {}, |_| {});
    h.wait(200);
    assert!(h.has("obs-node:file:alpha/a.go"));
    let alpha = h.bounds("obs-node:dir:alpha").expect("alpha");
    let beta = h.bounds("obs-node:dir:beta").expect("beta");
    assert!(beta.top() >= alpha.bottom());
}

#[gpui::test]
async fn an_explicit_scope_choice_is_remembered_per_loop(cx: &mut TestAppContext) {
    let list = vec!["src/main.go".to_string()];
    let mut h = mount(cx, changed_opts(list), |_| {}, |_| {});
    h.click("obs-scope:all");
    assert!(h.read(|o, _| o.browse_all));
    h.switch_to(PROJECT, "LOOP-2229");
    h.switch_to(PROJECT, WS);
    assert!(h.read(|o, _| o.browse_all));
    h.click("obs-scope:changed");
    h.switch_to(PROJECT, "LOOP-2229");
    h.switch_to(PROJECT, WS);
    assert!(!h.read(|o, _| o.browse_all));
}

fn pkg_tree(path: &str) -> Vec<TreeEntry> {
    if path.is_empty() {
        tree(&[("pkg", "pkg", true)])
    } else {
        Vec::new()
    }
}

fn two_worktrees() -> Overrides {
    let files_for = |ws: &str| {
        if ws == "LOOP-2229" {
            "pkg/beta_only.go"
        } else {
            "pkg/alpha_only.go"
        }
    };
    Overrides::default()
        .list_tree(|_, (_, _, path)| Ok(pkg_tree(&path)))
        .list_changed_files(move |_, ws| Ok(strings(&[files_for(&ws)])))
        .git_status(move |_, (_, ws)| Ok(statuses(&[(files_for(&ws), "m")])))
        .working_status(move |_, (_, ws)| Ok(statuses(&[(files_for(&ws), "m")])))
}

#[gpui::test]
async fn switching_worktree_never_shows_the_previous_ones_files(cx: &mut TestAppContext) {
    // B's reads stay in flight until released: a slow worktree, past the skeleton delay.
    // Each held read answers empty when released.
    type Pending = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;
    fn held<T: Default + Send + 'static>(
        pending: &Pending,
    ) -> recur_services::fake::Answer<Result<T>> {
        let (r, rel) = hold::<Result<T>>();
        pending
            .lock()
            .push(Box::new(move || rel.send(Ok(T::default()))));
        r
    }
    let pending: Pending = Arc::default();
    let (p1, p2, p3) = (pending.clone(), pending.clone(), pending.clone());
    let opts = two_worktrees()
        .list_tree(move |_, (_, ws, path)| {
            if ws == "LOOP-2229" {
                return held(&p1);
            }
            Ok(pkg_tree(&path)).into()
        })
        .list_changed_files(move |_, ws| {
            if ws == "LOOP-2229" {
                return held(&p2);
            }
            Ok(strings(&["pkg/alpha_only.go"])).into()
        })
        .git_status(move |_, (_, ws)| {
            if ws == "LOOP-2229" {
                return held(&p3);
            }
            Ok(statuses(&[("pkg/alpha_only.go", "m")])).into()
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.wait(100);
    assert!(h.has("obs-node:file:pkg/alpha_only.go"));
    h.switch_to(PROJECT, "LOOP-2229");
    h.wait(400);
    assert!(h.has("obs-skel-row:0"));
    assert!(!h.has("obs-node:file:pkg/alpha_only.go"));
    for release in pending.lock().drain(..) {
        release();
    }
    h.wait(100);
    assert!(!h.has("obs-node:file:pkg/alpha_only.go"));
}

#[gpui::test]
async fn a_fast_worktree_switch_shows_no_skeleton_at_all(cx: &mut TestAppContext) {
    let mut h = mount(cx, two_worktrees(), |_| {}, |_| {});
    h.wait(100);
    assert!(h.has("obs-node:file:pkg/alpha_only.go"));
    h.switch_to(PROJECT, "LOOP-2229");
    let mut saw_skeleton = false;
    let mut saw_stale = false;
    for _ in 0..12 {
        saw_skeleton |= h.has("obs-skel-row:0");
        saw_stale |= h.has("obs-node:file:pkg/alpha_only.go");
        h.wait(100);
    }
    assert!(h.has("obs-node:file:pkg/beta_only.go"));
    assert!(!saw_skeleton);
    assert!(!saw_stale);
}

#[gpui::test]
async fn clicking_a_changed_submodule_directory_opens_diff_without_error_toasts(
    cx: &mut TestAppContext,
) {
    const SUB: &str = "packages/contracts/lib/openzeppelin-contracts-upgradeable";
    let outlines = Calls::default();
    let o2 = outlines.clone();
    let opts = changed_opts(vec![SUB.to_string()])
        .read_file(|_, (_, _, path)| {
            if path == SUB {
                return Err(Error::new(format!("{SUB} is a directory")));
            }
            Ok(file(&path, "go", "package main\n", true))
        })
        .outline(move |_, (p, ws, path)| {
            push(&o2, &[&p, &ws, &path]);
            if path == SUB {
                return Err(Error::new("should not outline a directory"));
            }
            Ok(Vec::new())
        })
        .file_at_base(|_, (_, _, path)| {
            Ok(if path == SUB { "Subproject commit abcdef1\n" } else { "" }.into())
        });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.wait(100);
    h.click(&format!("obs-node:file:{SUB}"));
    h.wait(100);
    assert_eq!(
        h.read(|o, _| o.view_mode),
        recur_editor::view_state::EditorViewMode::Diff
    );
    assert!(h.read(|o, _| o.merge_editor.is_some()));
    assert!(h.toasts().iter().all(|(k, _)| *k != ToastKind::Error));
    assert!(outlines.lock().is_empty());
}

#[gpui::test]
async fn gitignored_entries_are_muted_and_tracked_dotfiles_are_not(cx: &mut TestAppContext) {
    let opts = Overrides::default().list_tree(|_, (_, _, path)| {
        if !path.is_empty() {
            return Ok(Vec::new());
        }
        let mut rows = tree(&[
            (".gitignore", ".gitignore", false),
            ("TODO.md", "TODO.md", false),
            ("dist", "dist", true),
            ("src", "src", true),
        ]);
        rows[1].ignored = true;
        rows[2].ignored = true;
        Ok(rows)
    });
    let mut h = mount(cx, opts, |_| {}, |_| {});
    h.click("obs-scope:all");
    assert!(h.has("obs-ignored:TODO.md"));
    assert!(h.has("obs-ignored:dist"));
    assert!(h.has("obs-node:file:.gitignore") && !h.has("obs-ignored:.gitignore"));
    assert!(h.has("obs-node:dir:src") && !h.has("obs-ignored:src"));
}
