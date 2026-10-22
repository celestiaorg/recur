//! Parity scenes: the `review` list in `rust/tools/ui-parity/scenes.json`, built the way
//! `web/scenes/Review.svelte` mounts ReviewSurface and a PR-mode DiffFile, over
//! `content/review.json`.

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled,
    Window, div,
};
use recur_domain::{FileDiff, Finding, Pr, PrComment};
use recur_services::mock::{MockBridge, MockOptions, Reply};
use recur_ui::gallery::SceneSpec;
use recur_ui::style::body;
use recur_view_core::{AppState, LocalStorage};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::diff_file::NoteMode;
use crate::review_diff::{DiffLayout, ReviewTab};
use crate::stack::{CardProps, FileStack};
use crate::surface::{ReviewSurface, SurfaceProps};

const SCENES_JSON: &str = include_str!("../../../../tools/ui-parity/scenes.json");
const FIXTURES: &str = include_str!("../../../../tools/ui-parity/content/review.json");

#[derive(Deserialize)]
struct Specs {
    #[serde(default)]
    review: Vec<SceneSpec>,
}

/// Every review scene, in file order.
pub fn specs() -> Vec<SceneSpec> {
    serde_json::from_str::<Specs>(SCENES_JSON)
        .map(|s| s.review)
        .unwrap_or_default()
}

pub fn exists(name: &str) -> bool {
    specs().iter().any(|s| s.name == name)
}

fn fixtures() -> Value {
    serde_json::from_str(FIXTURES).unwrap_or(Value::Null)
}

/// The large scene's files (the e2e "large mixed-size diff" shape).
pub fn large_files(count: usize) -> Value {
    let files: Vec<Value> = (0..count)
        .map(|i| {
            let n = if i % 11 == 0 { 400 } else { 2 + (i * 7) % 15 };
            let mut lines = vec![json!({"old": "", "new": "", "type": "hunk", "codeHtml": format!("@@ -1,{n} +1,{n} @@")})];
            for k in 1..=n {
                let t = if k % 3 == 0 {
                    "add"
                } else if k % 5 == 0 {
                    "del"
                } else {
                    "ctx"
                };
                let width = if (k + i) % 4 == 0 { 40 } else { 6 };
                lines.push(json!({
                    "old": if t == "add" { String::new() } else { k.to_string() },
                    "new": if t == "del" { String::new() } else { k.to_string() },
                    "type": t,
                    "codeHtml": format!("l{k} f{i} {}", "word ".repeat(width)),
                }));
            }
            json!({
                "path": format!("src/pkg{}/file{i}.ts", i % 9),
                "status": "m",
                "diff": {"add": n / 3, "del": n / 5},
                "lines": lines,
            })
        })
        .collect();
    Value::Array(files)
}

/// The review scenes' mock: `content/review.json` answers the review calls.
pub fn review_bridge(large: bool) -> std::sync::Arc<MockBridge> {
    let fx = fixtures();
    let files = if large {
        large_files(fx["large"]["files"].as_u64().unwrap_or(40) as usize)
    } else {
        fx["files"].clone()
    };
    let dirty: Vec<String> = serde_json::from_value(fx["dirty"].clone()).unwrap_or_default();
    let working: Vec<Value> = fx["files"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|f| dirty.iter().any(|d| f["path"] == d.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let commits = fx["commits"].clone();
    let commit_files = json!([fx["files"][2].clone()]);
    let base = json!({ "files": files });
    MockBridge::fresh(
        MockOptions::default()
            .resolver("DiffVsBase", move |_, _, _| Reply::value(base.clone()))
            .resolver("DiffWorking", move |_, _, _| {
                Reply::value(json!({ "files": working.clone() }))
            })
            .resolver("ListCommits", move |_, _, _| Reply::value(commits.clone()))
            .resolver("CommitDiff", move |_, _, _| {
                Reply::value(commit_files.clone())
            }),
    )
}

/// `findingsByPath` in fixture order.
pub fn findings_by_path() -> Vec<(String, Vec<Finding>)> {
    let list: Vec<Finding> =
        serde_json::from_value(fixtures()["findings"].clone()).unwrap_or_default();
    let mut out: Vec<(String, Vec<Finding>)> = Vec::new();
    for f in list {
        match out.iter_mut().find(|(p, _)| *p == f.file) {
            Some((_, v)) => v.push(f),
            None => out.push((f.file.clone(), vec![f])),
        }
    }
    out
}

/// The Observatory's side of the surface: it owns `tab` and follows `onTabChange`.
struct SurfaceScene(
    Entity<ReviewSurface>,
    #[allow(dead_code)] gpui_kit::Subscription,
);

impl SurfaceScene {
    fn new(surface: Entity<ReviewSurface>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe_in(
            &surface,
            window,
            |_, s, ev: &crate::surface::SurfaceEvent, window, cx| {
                if let crate::surface::SurfaceEvent::TabChange(t) = ev {
                    let t = *t;
                    s.update(cx, |s, cx| {
                        let mut p = s.props().clone();
                        p.tab = t;
                        s.set_props(p, window, cx);
                    });
                }
            },
        );
        SurfaceScene(surface, sub)
    }
}

impl Render for SurfaceScene {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        body(cx).size_full().flex().flex_col().child(self.0.clone())
    }
}

/// PR detail's files pane around one `.cfile` (`.prpane.flexpane > .diffwrap.no-sidebar >
/// .diffscroll`).
pub struct ThreadScene {
    stack: FileStack,
}

impl ThreadScene {
    pub fn new(state: Entity<AppState>, file: Rc<FileDiff>, cx: &mut App) -> Self {
        let mut stack = FileStack::new(state, cx);
        stack.set_files(vec![file]);
        stack.set_props(CardProps {
            layout: DiffLayout::Unified,
            note_mode: Some(NoteMode::Pr),
            comments: true,
            collapsible: true,
            ..Default::default()
        });
        ThreadScene { stack }
    }
}

impl Render for ThreadScene {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let stack = self.stack.render(
            Vec::new(),
            None,
            |s: &mut ThreadScene| &mut s.stack,
            Rc::new(|s: &mut ThreadScene, path, next, cx| {
                s.stack.toggle_collapsed(&path, next);
                cx.notify();
            }),
            window,
            cx,
        );
        body(cx)
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().flex().flex_col().child(stack))
    }
}

pub fn build(name: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
    let spec = specs().into_iter().find(|s| s.name == name)?;
    let variant = name.split_once('/').map_or("", |(_, v)| v);
    let variant = variant.strip_suffix("-paper").unwrap_or(variant);
    cx.set_global(crate::store::ReviewStore::default());
    let bridge = review_bridge(variant == "large");
    let services = bridge.services();
    let mut storage = LocalStorage::in_memory();
    storage.set("recur.theme", spec.theme.clone());
    if variant == "split" {
        storage.set("recur.prDiffLayout", "split");
    }
    if variant == "filelist" {
        storage.set("recur.reviewFileListMode", "flat");
    }
    let state: Entity<AppState> = AppState::install(services, storage, cx);
    if variant == "thread" {
        let fx = fixtures();
        let replies: Vec<PrComment> =
            serde_json::from_value(fx["threadReplies"].clone()).unwrap_or_default();
        let mut pr: Pr = bridge
            .with_seed(|s| s.pr_by_id("482").cloned())
            .and_then(|v| serde_json::from_value(v).ok())?;
        pr.comments.get_or_insert_with(Vec::new).extend(replies);
        let file = pr
            .files
            .as_ref()?
            .iter()
            .find(|f| f.path == "src/auth/oauth.ts")?
            .clone();
        state.update(cx, |s, cx| {
            s.prs = vec![pr];
            s.selected_pr_id = "482".into();
            cx.notify();
        });
        return Some(
            cx.new(|cx| ThreadScene::new(state, Rc::new(file), cx))
                .into(),
        );
    }
    let fx = fixtures();
    let large = variant == "large";
    let props = SurfaceProps {
        project_id: "storefront".into(),
        workspace_id: "LOOP-2231".into(),
        base_label: "main".into(),
        active: true,
        tab: if variant == "commits" {
            ReviewTab::Commits
        } else {
            ReviewTab::Changes
        },
        count: if large {
            fx["large"]["files"].as_u64().unwrap_or(40) as usize
        } else {
            fx["files"].as_array().map_or(0, Vec::len)
        },
        findings_by_path: (!large).then(|| Rc::new(findings_by_path())),
        dirty_paths: (!large).then(|| {
            Rc::new(
                serde_json::from_value::<HashSet<String>>(fx["dirty"].clone()).unwrap_or_default(),
            )
        }),
        working_dirty_count: if variant == "commits" { 2 } else { 0 },
        ..Default::default()
    };
    let surface = cx.new(|cx| ReviewSurface::new(state, props, window, cx));
    surface.update(cx, |s, cx| {
        s.set_add_finding(
            Some(Rc::new(|_, _, _, _, _, _| gpui_kit::Task::ready(true))),
            window,
            cx,
        )
    });
    Some(cx.new(|cx| SurfaceScene::new(surface, window, cx)).into())
}
