//! Windows around the review surface on the mock bridge, as the Observatory and PR detail
//! host it, and selector-driven input (`debug_selector` keys).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{Entity, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px};
use gpui_kit::{
    AppContext as _, Bounds, Context, IntoElement, ParentElement, Pixels, Render, Styled,
    Subscription, Window, div,
};
use recur_domain::{FileDiff, Finding, Pr};
use recur_engine::view::comment_thread::unresolved_line_notes_by_path;
use recur_services::Services;
use recur_services::mock::{MockBridge, MockOptions};
use recur_view_core::{AppState, LocalStorage};

use crate::diff_file::{DiffFile, DiffFileHooks};
use crate::file_list::{FileListProps, ReviewFileList};
use crate::findings::FindingsByPath;
use crate::review_diff::{DiffLayout, FileListMode, ReviewTab};
use crate::stack::{CardProps, FileStack};
use crate::store::ReviewStore;
use crate::surface::{ReviewSurface, SurfaceEvent, SurfaceProps};

pub const WS: &str = "LOOP-2231";
pub const PROJECT: &str = "storefront";

pub fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        recur_ui::init(cx);
        cx.set_global(ReviewStore::default());
    });
}

/// The Observatory's side of the surface: it owns `tab`, follows `onTabChange`, and saves
/// review line notes as findings (`addReviewFinding`), re-listing them into `findingsByPath`.
pub struct SurfaceHost {
    pub surface: Entity<ReviewSurface>,
    pub events: Rc<RefCell<Vec<SurfaceEvent>>>,
    pub services: Services,
    _sub: Subscription,
}

impl SurfaceHost {
    fn on_event(&mut self, ev: &SurfaceEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.events.borrow_mut().push(ev.clone());
        if let SurfaceEvent::TabChange(t) = ev {
            let t = *t;
            self.surface.update(cx, |s, cx| {
                let mut p = s.props().clone();
                p.tab = t;
                s.set_props(p, window, cx);
            });
        }
    }

    pub fn set(
        &mut self,
        f: impl FnOnce(&mut SurfaceProps),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.surface.update(cx, |s, cx| {
            let mut p = s.props().clone();
            f(&mut p);
            s.set_props(p, window, cx);
        });
    }
}

impl Render for SurfaceHost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::style::window_root(
            recur_ui::style::body(cx)
                .size_full()
                .flex()
                .flex_col()
                .child(self.surface.clone()),
        )
    }
}

/// Findings grouped by file.
pub fn by_path(list: &[Finding]) -> Vec<(String, Vec<Finding>)> {
    let mut out: Vec<(String, Vec<Finding>)> = Vec::new();
    for f in list
        .iter()
        .filter(|f| f.status.is_empty() || f.status == "open")
    {
        match out.iter_mut().find(|(p, _)| *p == f.file) {
            Some((_, v)) => v.push(f.clone()),
            None => out.push((f.file.clone(), vec![f.clone()])),
        }
    }
    out
}

pub struct H<'a> {
    pub state: Entity<AppState>,
    pub cx: &'a mut VisualTestContext,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

impl H<'_> {
    pub fn settle(&mut self) {
        for _ in 0..4 {
            self.cx.run_until_parked();
            self.cx.update(|window, _| window.refresh());
        }
        self.cx.run_until_parked();
    }

    pub fn bounds(&mut self, sel: &str) -> Option<Bounds<Pixels>> {
        self.settle();
        self.cx.debug_bounds(leak(sel))
    }

    pub fn has(&mut self, sel: &str) -> bool {
        self.bounds(sel).is_some()
    }

    pub fn click(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.cx.simulate_click(p, Modifiers::none());
        self.settle();
    }

    pub fn hover(&mut self, sel: &str) {
        let p = self
            .bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
            .center();
        self.cx
            .simulate_mouse_move(point(p.x + px(1.), p.y), None, Modifiers::none());
        self.cx.simulate_mouse_move(p, None, Modifiers::none());
        self.settle();
    }

    /// Press on `from`, move to `to`, release (`dragTo`).
    pub fn drag(&mut self, from: &str, to: &str) {
        let a = self
            .bounds(from)
            .unwrap_or_else(|| panic!("no element {from}"))
            .center();
        let b = self
            .bounds(to)
            .unwrap_or_else(|| panic!("no element {to}"))
            .center();
        self.cx.simulate_mouse_move(a, None, Modifiers::none());
        self.cx
            .simulate_mouse_down(a, MouseButton::Left, Modifiers::none());
        self.settle();
        let steps = 4;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let p = point(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t);
            self.cx
                .simulate_mouse_move(p, Some(MouseButton::Left), Modifiers::none());
            self.settle();
        }
        self.cx
            .simulate_mouse_up(b, MouseButton::Left, Modifiers::none());
        self.settle();
    }

    /// Type into the focused field, one character at a time.
    pub fn type_text(&mut self, s: &str) {
        for ch in s.chars() {
            self.cx.simulate_input(&ch.to_string());
            self.cx.run_until_parked();
        }
        self.settle();
    }

    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
    }

    pub fn wait(&mut self, ms: u64) {
        self.cx.executor().advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    pub fn pr(&mut self, id: &str) -> Pr {
        let state = self.state.clone();
        state
            .read_with(self.cx, |s, _| s.pr(id).cloned())
            .expect("pr")
    }
}

/// A surface on `opts`, as the Observatory mounts it for the seed's first active workspace.
pub fn surface<'a>(
    cx: &'a mut TestAppContext,
    opts: MockOptions,
    props: impl FnOnce(&mut SurfaceProps),
) -> (H<'a>, Entity<SurfaceHost>) {
    init(cx);
    let bridge = MockBridge::fresh(opts);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), LocalStorage::in_memory(), cx));
    let mut p = SurfaceProps {
        project_id: PROJECT.into(),
        workspace_id: WS.into(),
        base_label: "main".into(),
        active: true,
        tab: ReviewTab::Changes,
        count: 1,
        working_dirty_count: 1,
        ..Default::default()
    };
    props(&mut p);
    let st = state.clone();
    let (host, vcx) = cx.add_window_view(move |window, cx| {
        let surface = cx.new(|cx| ReviewSurface::new(st, p, window, cx));
        let events: Rc<RefCell<Vec<SurfaceEvent>>> = Rc::default();
        let sub = cx.subscribe_in(
            &surface,
            window,
            |h: &mut SurfaceHost, _, ev, window, cx| h.on_event(ev, window, cx),
        );
        SurfaceHost {
            surface,
            events,
            services: services.clone(),
            _sub: sub,
        }
    });
    let host2 = host.clone();
    // `addReviewFinding`: CreateFinding, then reload the findings into `findingsByPath`.
    vcx.update(|window, cx| {
        let services = host2.read(cx).services.clone();
        let h = host2.downgrade();
        let add: crate::diff_file::AddFindingFn = Rc::new(move |path, line, body, end, _, cx| {
            let services = services.clone();
            let h = h.clone();
            cx.spawn(async move |cx| {
                if services
                    .observatory
                    .create_finding(WS, &path, line, body.trim(), end)
                    .await
                    .is_err()
                {
                    return false;
                }
                let list = services
                    .observatory
                    .list_findings(WS)
                    .await
                    .unwrap_or_default();
                cx.update(|cx| {
                    let Some(h) = h.upgrade() else { return };
                    let surface = h.read(cx).surface.clone();
                    let _ = cx.update_window(cx.windows()[0], |_, window, cx| {
                        surface.update(cx, |s, cx| {
                            let mut p = s.props().clone();
                            p.findings_by_path = Some(Rc::new(by_path(&list)));
                            s.set_props(p, window, cx);
                        });
                    });
                });
                true
            })
        });
        let surface = host2.read(cx).surface.clone();
        surface.update(cx, |s, cx| s.set_add_finding(Some(add), window, cx));
    });
    vcx.simulate_resize(gpui::size(px(1280.), px(800.)));
    vcx.update(|window, _| window.activate_window());
    let mut h = H { state, cx: vcx };
    h.settle();
    (h, host)
}

/// An `onOpenFile(path, line?)` call.
pub type OpenedFile = (String, Option<i64>);

/// PR detail's Files tab for one PR: the file list with unresolved-comment flags beside the
/// stacked diffs, in PR note mode.
pub struct PrHost {
    pub stack: FileStack,
    pub list: Entity<ReviewFileList>,
    pub highlight: i64,
    pub findings: Option<Rc<FindingsByPath>>,
    pub opened: Rc<RefCell<Vec<OpenedFile>>>,
    pub opened_findings: Rc<RefCell<Vec<Finding>>>,
    state: Entity<AppState>,
    pr_id: String,
}

impl PrHost {
    fn files(&self, cx: &gpui_kit::App) -> Vec<Rc<FileDiff>> {
        self.state
            .read(cx)
            .pr(&self.pr_id)
            .and_then(|p| p.files.clone())
            .unwrap_or_default()
            .into_iter()
            .map(Rc::new)
            .collect()
    }

    pub fn select(&mut self, path: &str, highlight: Option<i64>) {
        self.highlight = highlight.unwrap_or(0);
        self.stack.select_path(path);
    }

    pub fn diff_file(&self, path: &str) -> Option<Entity<DiffFile>> {
        self.stack.diff_file(path).cloned()
    }
}

impl Render for PrHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.files(cx);
        self.stack.set_files(files.clone());
        let opened = self.opened.clone();
        let opened_f = self.opened_findings.clone();
        self.stack.set_props(CardProps {
            layout: DiffLayout::Unified,
            note_mode: None,
            comments: true,
            collapsible: true,
            open_file_title: Some("Open in workspace".into()),
            findings_by_path: self.findings.clone(),
            highlight_line: self.highlight,
            hooks: DiffFileHooks {
                open_file: Some(Rc::new(move |p, l, _, _| opened.borrow_mut().push((p, l)))),
                open_finding: Some(Rc::new(move |f, _, _| opened_f.borrow_mut().push(f))),
                ..Default::default()
            },
            ..Default::default()
        });
        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let comments = self
            .state
            .read(cx)
            .pr(&self.pr_id)
            .and_then(|p| p.comments.clone())
            .unwrap_or_default();
        let by: Vec<(String, Vec<recur_domain::PrComment>)> =
            unresolved_line_notes_by_path(&comments, &paths)
                .into_iter()
                .map(|(p, v)| (p, v.into_iter().cloned().collect()))
                .collect();
        let props = FileListProps {
            files,
            selected_path: self.stack.selected().to_string(),
            mode: FileListMode::Tree,
            findings_by_path: self.findings.clone(),
            comments_by_path: Some(Rc::new(by)),
            dirty_paths: None,
            reveal: self.stack.toc_reveal,
        };
        self.list.update(cx, |l, cx| l.set_props(props, cx));
        let stack = self.stack.render(
            Vec::new(),
            None,
            |h: &mut PrHost| &mut h.stack,
            Rc::new(|h: &mut PrHost, path, next, cx| {
                h.stack.toggle_collapsed(&path, next);
                cx.notify();
            }),
            window,
            cx,
        );
        recur_ui::style::window_root(
            recur_ui::style::body(cx)
                .size_full()
                .flex()
                .child(div().w(px(250.)).flex().flex_col().child(self.list.clone()))
                .child(div().flex_1().min_w_0().flex().flex_col().child(stack)),
        )
    }
}

/// PR #482 (the seed's richest PR) in PR detail's Files tab.
pub fn pr_files<'a>(
    cx: &'a mut TestAppContext,
    opts: MockOptions,
    findings: Option<Vec<Finding>>,
) -> (H<'a>, Entity<PrHost>) {
    init(cx);
    let bridge = MockBridge::fresh(opts);
    let services = bridge.services();
    let state = cx.update(|cx| AppState::install(services.clone(), LocalStorage::in_memory(), cx));
    let prs: Vec<Pr> = bridge.with_seed(|s| {
        s.prs()
            .filter_map(|v| serde_json::from_value(v.clone()).ok())
            .collect()
    });
    let full: Pr = bridge
        .with_seed(|s| s.pr_by_id("482").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .expect("seed pr 482");
    cx.update(|cx| {
        state.update(cx, |s, cx| {
            s.prs = prs;
            s.upsert_pr(full, cx);
            s.selected_pr_id = "482".into();
            cx.notify();
        })
    });
    let st = state.clone();
    let (host, vcx) = cx.add_window_view(move |_, cx| {
        let me = cx.entity().downgrade();
        let list = cx.new(|_| {
            ReviewFileList::new(
                FileListProps::default(),
                Some(Rc::new(
                    move |path: String, hl, _, cx: &mut gpui_kit::App| {
                        let me = me.clone();
                        cx.defer(move |cx| {
                            let _ = me.update(cx, |h: &mut PrHost, cx| {
                                h.select(&path, hl);
                                cx.notify();
                            });
                        });
                    },
                )),
            )
        });
        PrHost {
            stack: FileStack::new(st.clone(), cx),
            list,
            highlight: 0,
            findings: findings.map(|f| Rc::new(by_path(&f))),
            opened: Rc::default(),
            opened_findings: Rc::default(),
            state: st,
            pr_id: "482".into(),
        }
    });
    vcx.simulate_resize(gpui::size(px(1280.), px(900.)));
    vcx.update(|window, _| window.activate_window());
    let mut h = H { state, cx: vcx };
    h.settle();
    (h, host)
}
