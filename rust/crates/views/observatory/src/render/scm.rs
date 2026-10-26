//! The Source Control panel (`.obs-scm-panel`): staged and unstaged changes, flat or as a
//! tree with sticky folders, stage / unstage / discard per row or folder, and the commit
//! composer (generate message, ⌘↵ to commit).
//!
//! Rows lay out in flow; each open folder records its content offset and its branch's end
//! (probes from the last frame), and folders scrolled past paint again in a sticky layer.

use recur_ui::guard::Guarded;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, px,
};
use recur_code::view::changed_tree::{
    build_tree_from_paths, cap_visible_tree, flatten_visible_tree, paths_under_dir,
};
use recur_domain::TreeEntry;
use recur_ui::exact::vgap;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::scroll_area;
use recur_ui::{IconName, Theme, icon};
use recur_view_review::review_diff::FileListMode;
use recur_view_review::style::hsla;

use crate::obs::{LIST_CAP, Observatory};
use crate::style::obs_empty;

const ROW_H: f32 = 24.;
const FADE: f32 = 20.;

/// Probed positions: an open folder's row top and its branch end, in content coordinates.
#[derive(Clone, Default)]
pub(crate) struct ScmProbes {
    pub rows: Rc<RefCell<HashMap<String, (f32, f32)>>>,
    pub ends: Rc<RefCell<HashMap<String, f32>>>,
    pub body: Rc<std::cell::Cell<Bounds<Pixels>>>,
}

fn st_color(st: &str, t: &recur_ui::Tokens) -> recur_ui::Color {
    match st {
        "m" => t.git_m,
        "a" | "u" => t.git_u,
        "d" => t.git_d,
        _ => t.text,
    }
}

impl Observatory {
    fn scm_stage_btn(
        &self,
        id: String,
        staged: bool,
        title: String,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = Theme::of(cx).tokens;
        let _ = title;
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded(px(4.))
            .cursor_pointer()
            .text_color(hsla(if staged { t.git_a } else { t.faint }))
            .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane)))
            .child(
                icon(if staged {
                    IconName::Minus
                } else {
                    IconName::Plus
                })
                .size(px(12.))
                .stroke_width(2.),
            )
    }

    fn scm_discard_btn(
        &self,
        id: String,
        show: bool,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = Theme::of(cx).tokens;
        div()
            .id(SharedString::from(id))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(18.))
            .rounded(px(4.))
            .cursor_pointer()
            .text_color(hsla(t.coral))
            .when(!show, |d| d.opacity(0.))
            .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane)))
            .child(icon(IconName::Undo).size(px(12.)))
    }

    /// A file row (`scmRow`): stage, status letter, name, folder, discard.
    fn scm_file_row(
        &self,
        path: &str,
        staged: bool,
        depth: usize,
        show_dir: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let st = self
            .working_status
            .get(path)
            .cloned()
            .unwrap_or_else(|| "?".into());
        let key = format!("{}:{path}", if staged { "s" } else { "u" });
        let hovered = self.scm_hovered.as_deref() == Some(key.as_str());
        let name = path.rsplit('/').next().unwrap_or(path).to_string();
        let dir = path.rfind('/').map(|i| path[..i].to_string());
        let (p1, p2, p3) = (path.to_string(), path.to_string(), path.to_string());
        let stage = titled(
            SharedString::from(format!("scm-stage-t-{key}")),
            if staged { "Unstage" } else { "Stage" },
            self.scm_stage_btn(format!("scm-stage-{key}"), staged, String::new(), cx)
                .debug_selector({
                    let k = key.clone();
                    move || format!("scm-stage:{k}")
                })
                .on_click(cx.guarded_listener(move |o, _, _, cx| {
                    o.stage_paths(vec![p1.clone()], !staged, cx)
                })),
        );
        let mut file = div()
            .id(SharedString::from(format!("scm-file-{key}")))
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .child(
                div().w(px(12.)).flex_none().flex().justify_center().child(
                    text(st.to_uppercase())
                        .size(10.)
                        .weight(700.)
                        .tracking(0.02)
                        .line_height(LineHeight::Scale(1.35))
                        .color(st_color(&st, &t))
                        .nowrap(),
                ),
            );
        let wide_name = !(show_dir && dir.is_some());
        let name_el = text(name)
            .size(12.5)
            .line_height(LineHeight::Scale(1.35))
            .color(
                if hovered && self.scm_name_hover.as_deref() == Some(key.as_str()) {
                    t.amber
                } else {
                    t.text
                },
            )
            .truncate();
        file = if wide_name {
            file.child(div().flex_1().min_w_0().child(name_el))
        } else {
            file.child(
                div()
                    .flex_none()
                    .max_w(gpui_kit::relative(0.55))
                    .min_w_0()
                    .child(name_el),
            )
        };
        if show_dir && let Some(d) = dir {
            file = file.child(
                div().flex_1().min_w_0().flex().justify_end().child(
                    text(d)
                        .size(11.)
                        .line_height(LineHeight::Scale(1.35))
                        .color(t.faint)
                        .truncate(),
                ),
            );
        }
        let hk = key.clone();
        let file = titled(
            SharedString::from(format!("scm-file-t-{key}")),
            path.to_string(),
            file.on_hover(cx.guarded_listener(move |o, on: &bool, _, cx| {
                o.scm_name_hover = on.then(|| hk.clone());
                cx.notify();
            }))
            .on_click(cx.guarded_listener(move |o, _, window, cx| {
                o.open_path(
                    crate::obs::OpenRequest::new(p2.clone()).prefer_diff(true),
                    window,
                    cx,
                )
                .detach()
            })),
        );
        let discard = titled(
            SharedString::from(format!("scm-discard-t-{key}")),
            "Discard changes",
            self.scm_discard_btn(format!("scm-discard-{key}"), hovered, cx)
                .debug_selector({
                    let k = key.clone();
                    move || format!("scm-discard:{k}")
                })
                .on_click(cx.guarded_listener(move |o, _, window, cx| {
                    let p = p3.clone();
                    o.discard_paths(vec![p.clone()], &p, window, cx)
                })),
        );
        let hk = key.clone();
        div()
            .id(SharedString::from(format!("scm-row-{key}")))
            .flex()
            .items_center()
            .gap(px(2.))
            .pl(px(10. + depth as f32 * 12.))
            .pr(px(10.))
            .py(px(3.))
            .when(hovered, |d| d.bg(hsla(t.pane_2)))
            .on_hover(cx.guarded_listener(move |o, on: &bool, _, cx| {
                if *on {
                    o.scm_hovered = Some(hk.clone());
                } else if o.scm_hovered.as_deref() == Some(hk.as_str()) {
                    o.scm_hovered = None;
                }
                cx.notify();
            }))
            .child(stage)
            .child(file)
            .child(discard)
            .into_any_element()
    }

    /// A folder row (`scmTreeRow` dir): stage all under it, chevron + name, discard all.
    fn scm_dir_row(
        &self,
        e: &TreeEntry,
        staged: bool,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<gpui_kit::Div> {
        let t = Theme::of(cx).tokens;
        let open = !self.scm_collapsed.contains(&e.path);
        let wp = self.working_paths();
        let files = paths_under_dir(&e.path, if staged { &wp.0 } else { &wp.1 });
        let n = files.len();
        let key = format!("{}:{}/", if staged { "s" } else { "u" }, e.path);
        let hovered = self.scm_hovered.as_deref() == Some(key.as_str());
        let (f1, f2, path) = (files.clone(), files, e.path.clone());
        let label = if staged {
            format!("Unstage {n} file{}", if n == 1 { "" } else { "s" })
        } else {
            format!("Stage {n} file{}", if n == 1 { "" } else { "s" })
        };
        let stage = titled(
            SharedString::from(format!("scm-dstage-t-{key}")),
            label,
            self.scm_stage_btn(format!("scm-dstage-{key}"), staged, String::new(), cx)
                .on_click(
                    cx.guarded_listener(move |o, _, _, cx| o.stage_paths(f1.clone(), !staged, cx)),
                ),
        );
        let p2 = path.clone();
        let toggle = titled(
            SharedString::from(format!("scm-dir-t-{key}")),
            e.path.clone(),
            div()
                .id(SharedString::from(format!("scm-dir-{key}")))
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(4.))
                .cursor_pointer()
                .child(
                    icon(IconName::ChevronDown)
                        .size(px(12.))
                        .stroke_width(2.4)
                        .color(hsla(t.faint))
                        .rotate(if open { 0. } else { -90. }),
                )
                .child(
                    div().flex_1().min_w_0().child(
                        text(e.name.clone())
                            .size(12.5)
                            .line_height(LineHeight::Scale(1.35))
                            .truncate(),
                    ),
                )
                .on_click(cx.guarded_listener(move |o, _, _, cx| o.toggle_scm_dir(&p2, cx))),
        );
        let p3 = path.clone();
        let discard = titled(
            SharedString::from(format!("scm-ddiscard-t-{key}")),
            "Discard changes",
            self.scm_discard_btn(format!("scm-ddiscard-{key}"), hovered, cx)
                .on_click(cx.guarded_listener(move |o, _, window, cx| {
                    o.discard_paths(f2.clone(), &p3, window, cx)
                })),
        );
        let hk = key.clone();
        div()
            .id(SharedString::from(format!("scm-drow-{key}")))
            .flex()
            .items_center()
            .gap(px(2.))
            .h(px(ROW_H))
            .pl(px(10. + depth as f32 * 12.))
            .pr(px(10.))
            .bg(hsla(if hovered { t.pane_2 } else { t.bg }))
            .on_hover(cx.guarded_listener(move |o, on: &bool, _, cx| {
                if *on {
                    o.scm_hovered = Some(hk.clone());
                } else if o.scm_hovered.as_deref() == Some(hk.as_str()) {
                    o.scm_hovered = None;
                }
                cx.notify();
            }))
            .child(stage)
            .child(toggle)
            .child(discard)
    }

    fn scm_tree(
        &self,
        nodes: &[TreeEntry],
        staged: bool,
        depth: usize,
        out: &mut Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) {
        for e in nodes {
            if !e.dir {
                out.push(self.scm_file_row(&e.path, staged, depth, false, cx));
                continue;
            }
            let key = format!("{}:{}", if staged { "s" } else { "u" }, e.path);
            let open = !self.scm_collapsed.contains(&e.path);
            let rows = self.scm_probes.rows.clone();
            let body = self.scm_probes.body.clone();
            let probe_key = key.clone();
            let row = self.scm_dir_row(e, staged, depth, cx).child(
                canvas(
                    move |b, _, _| {
                        let top = f32::from(b.origin.y - body.get().origin.y);
                        rows.borrow_mut()
                            .insert(probe_key.clone(), (top, depth as f32));
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_0(),
            );
            out.push(div().relative().child(row).into_any_element());
            if open {
                self.scm_tree(&e.children, staged, depth + 1, out, cx);
                let ends = self.scm_probes.ends.clone();
                let body = self.scm_probes.body.clone();
                let k = key.clone();
                out.push(
                    canvas(
                        move |b, _, _| {
                            ends.borrow_mut()
                                .insert(k.clone(), f32::from(b.origin.y - body.get().origin.y));
                        },
                        |_, _, _, _| {},
                    )
                    .w_full()
                    .h(px(0.))
                    .into_any_element(),
                );
            }
        }
    }

    fn scm_more(
        &self,
        key: &'static str,
        total: usize,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if total <= LIST_CAP || self.expanded_lists.contains(key) {
            return None;
        }
        Some(crate::render::panels::more_row(key, total, cx))
    }

    fn scm_section(&self, staged: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        let t = Theme::of(cx).tokens;
        let wp = self.working_paths();
        let paths = if staged { &wp.0 } else { &wp.1 };
        if paths.is_empty() {
            return None;
        }
        let key: &'static str = if staged { "staged" } else { "unstaged" };
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(2.))
            .child(
                text(if staged { "Staged" } else { "Changes" })
                    .size(11.)
                    .weight(600.)
                    .tracking(0.02)
                    .line_height(LineHeight::Scale(1.45))
                    .color(t.muted)
                    .nowrap(),
            )
            .child(
                text(paths.len().to_string())
                    .size(11.)
                    .weight(500.)
                    .tracking(0.02)
                    .line_height(LineHeight::Scale(1.45))
                    .color(t.faint)
                    .nowrap(),
            )
            .child(
                div()
                    .id(SharedString::from(format!("scm-all-{key}")))
                    .debug_selector(move || format!("scm-all:{key}"))
                    .ml_auto()
                    .cursor_pointer()
                    .text_color(hsla(t.muted))
                    .hover(move |s| s.text_color(hsla(t.text)))
                    .child(
                        text(if staged { "Unstage all" } else { "Stage all" })
                            .size(11.)
                            .weight(500.)
                            .tracking(0.02)
                            .line_height(LineHeight::Scale(1.45))
                            .nowrap(),
                    )
                    .on_click(cx.guarded_listener(move |o, _, _, cx| {
                        if staged {
                            o.unstage_all(cx)
                        } else {
                            o.stage_all(cx)
                        }
                    })),
            );
        let mut rows: Vec<AnyElement> = Vec::new();
        if self.scm_file_list_mode == FileListMode::Tree {
            let tree = build_tree_from_paths(paths);
            let cap = if self.expanded_lists.contains(key) {
                usize::MAX
            } else {
                LIST_CAP
            };
            let collapsed: std::collections::HashSet<String> =
                self.scm_collapsed.iter().cloned().collect();
            let capped = cap_visible_tree(&tree, &collapsed, Some(cap));
            self.scm_tree(&capped, staged, 0, &mut rows, cx);
            let total = flatten_visible_tree(&tree, &collapsed).len();
            rows.extend(self.scm_more(key, total, cx));
        } else {
            let n = if self.expanded_lists.contains(key) {
                paths.len()
            } else {
                paths.len().min(LIST_CAP)
            };
            for p in &paths[..n] {
                rows.push(self.scm_file_row(p, staged, 0, true, cx));
            }
            rows.extend(self.scm_more(key, paths.len(), cx));
        }
        Some(
            div()
                .mb(px(6.))
                .debug_selector(move || format!("scm-sec:{key}"))
                .child(head)
                .child(div().flex().flex_col().children(rows))
                .into_any_element(),
        )
    }

    /// The sticky layer: open folders scrolled past their natural row.
    fn scm_sticky_layer(
        &self,
        scroll: f32,
        cx: &mut Context<Self>,
        window: &mut Window,
    ) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let rows = self.scm_probes.rows.borrow().clone();
        let ends = self.scm_probes.ends.borrow().clone();
        let mut out: Vec<(usize, AnyElement)> = Vec::new();
        let wp = self.working_paths();
        for (key, (natural, depth)) in rows {
            let Some(end) = ends.get(&key).copied() else {
                continue;
            };
            let (staged, path) = match key.split_once(':') {
                Some(("s", p)) => (true, p.to_string()),
                Some((_, p)) => (false, p.to_string()),
                None => continue,
            };
            if self.scm_collapsed.contains(&path) {
                continue;
            }
            let content_top = natural + scroll;
            let want = scroll + 4. + depth * ROW_H;
            let top = want.min(end - ROW_H).max(content_top);
            if top <= content_top + 0.01 {
                continue;
            }
            let paths = if staged { &wp.0 } else { &wp.1 };
            let tree = build_tree_from_paths(paths);
            let Some(e) = crate::explorer::find_entry(&tree, &path).cloned() else {
                continue;
            };
            let fade_under = end - (top + ROW_H) > 0.5;
            let row = self.scm_dir_row(&e, staged, depth as usize, cx);
            let fade = fade_under.then(|| {
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(ROW_H))
                    .h(px(FADE))
                    .child(recur_ui::style::gradient_fill(
                        180.,
                        (t.bg, 0.),
                        (t.bg.with_alpha(0.), 1.),
                    ))
            });
            out.push((
                depth as usize,
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .flex_col()
                    .child(vgap(top - scroll, window))
                    .child(div().relative().child(row).children(fade))
                    .into_any_element(),
            ));
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.0));
        out.into_iter().map(|(_, e)| e).collect()
    }

    /// `.obs-scm-panel`.
    pub(crate) fn render_scm_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        // Tracks the zone for its fields; the panel itself is not focusable in the page.
        let root = div()
            .id("obs-scm-panel")
            .debug_selector(|| "obs-scm-panel".into())
            .track_focus(&self.scm_focus)
            .on_mouse_down(gpui_kit::MouseButton::Left, |_, window, _| {
                window.prevent_default()
            })
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .h_full();
        if self.worktree_missing {
            return root
                .child(obs_empty(
                    "This workspace's folder no longer exists.",
                    true,
                    cx,
                ))
                .into_any_element();
        }
        if !self.is_git(cx) {
            return root
                .child(obs_empty("Not a git repository.", true, cx))
                .into_any_element();
        }
        if !self.tree_writable() {
            return root
                .child(obs_empty(
                    "Open a loop worktree to stage and commit changes.",
                    true,
                    cx,
                ))
                .into_any_element();
        }
        if self.working_dirty_count() == 0 {
            return root
                .child(obs_empty("No uncommitted changes.", true, cx))
                .into_any_element();
        }
        // The changes list as its own cached view: a key typed in the commit message below it
        // redraws the panel's foot, not every row.
        let pane = self
            .scm_body_pane
            .get_or_insert_with(|| recur_ui::pane::Pane::of(cx, Self::render_scm_body))
            .clone();
        let body = recur_ui::style::cached_view(
            &pane,
            gpui_kit::StyleRefinement::default()
                .flex_1()
                .min_h_0()
                .w_full(),
            cx,
        );
        let foot = self
            .commit_foot
            .get_or_insert_with(|| recur_ui::isolated::Part::of(cx, Self::render_commit_foot))
            .clone();
        self.inputs.commit_view.redraws(foot.entity_id());
        let _ = (t, window);
        root.child(body).child(foot).into_any_element()
    }

    /// The staged and unstaged sections with their sticky headers.
    fn render_scm_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let scroll = -f32::from(self.scm_scroll.offset().y);
        let mut content = div().relative().flex().flex_col().pt(px(4.)).pb(px(6.));
        if let Some(s) = self.scm_section(true, cx) {
            content = content.child(s);
        }
        if let Some(s) = self.scm_section(false, cx) {
            content = content.child(s);
        }
        let layer = self.scm_sticky_layer(scroll, cx, window);
        let body_cell = self.scm_probes.body.clone();
        div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(
                canvas(move |b, _, _| body_cell.set(b), |_, _, _, _| {})
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
            )
            .child(
                scroll_area("obs-scm-body")
                    .overlay()
                    .track(&self.scm_scroll)
                    .child(content),
            )
            .child(div().absolute().inset_0().children(layer))
            .into_any_element()
    }

    /// `.obs-scm-foot`: the commit message composer.
    fn render_commit_foot(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let staged = self.staged_files.len();
        let busy = self.commit_busy;
        let msg_busy = self.commit_msg_busy;
        let msg = self.inputs.commit_msg(cx);
        let gen_disabled = msg_busy || staged == 0;
        let gen_title = if msg_busy {
            "Generating…".to_string()
        } else if staged > 0 {
            "Generate commit message with AI".to_string()
        } else {
            "Stage changes to generate a message".to_string()
        };
        let pulse = (msg_busy && !recur_ui::motion::reduced(cx))
            .then(|| crate::render::pulse_opacity(self.anim_epoch, window, cx));
        let gen_color = if msg_busy { t.amber } else { t.faint };
        let gen_btn =
            titled(
                "scm-gen-t",
                gen_title,
                div()
                    .id("scm-gen")
                    .debug_selector(|| "scm-gen".into())
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(24.))
                    .rounded(px(5.))
                    .text_color(hsla(gen_color))
                    .when(gen_disabled && !msg_busy, |d| d.opacity(0.35))
                    .when_some(pulse, |d, k| d.opacity(k))
                    .when(!gen_disabled, |d| {
                        d.cursor_pointer()
                            .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                            .on_click(cx.guarded_listener(|o, _, window, cx| {
                                o.generate_commit_msg(window, cx)
                            }))
                    })
                    .child(icon(IconName::Sparkle).size(px(13.))),
            );
        let can_commit = !busy && staged > 0 && !msg.trim().is_empty();
        let title = if staged > 0 {
            format!("Commit {staged} file{}", if staged == 1 { "" } else { "s" })
        } else {
            "Stage changes to commit".into()
        };
        let label = if busy { "Committing…" } else { "Commit" };
        let go_bg = if can_commit {
            t.text
        } else {
            recur_ui::style::faded(t.text, t.text, 0.35, t.bg)
        };
        let go_fg = if can_commit {
            t.bg
        } else {
            recur_ui::style::faded(t.bg, t.text, 0.35, t.bg)
        };
        let go = titled(
            "scm-commit-t",
            title,
            div()
                .id("scm-commit")
                .debug_selector(|| "scm-commit".into())
                .ml_auto()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(5.))
                .h(px(24.))
                .px(px(8.))
                .rounded(px(6.))
                .bg(hsla(go_bg))
                .text_color(hsla(go_fg))
                .child(text(label).size(11.).weight(600.).nowrap())
                .child(
                    recur_ui::kbd::kbd("⌘↵")
                        .size(11.)
                        .weight(500.)
                        .opacity(0.55),
                )
                .when(can_commit, |d| {
                    d.cursor_pointer().on_click(
                        cx.guarded_listener(|o, _, window, cx| o.human_commit(window, cx)),
                    )
                }),
        );
        let field = div()
            .min_h(px(56.))
            .max_h(px(120.))
            .pt(px(8.))
            .px(px(10.))
            .pb(px(4.))
            .text_size(px(12.5))
            .line_height(gpui_kit::relative(1.5))
            .when(busy, |d| d.opacity(0.55))
            .child(self.inputs.commit_view.element(18.75, cx));
        let compose = recur_ui::style::card(t.bg, t.line, 10.)
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(field)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pt(px(2.))
                    .px(px(8.))
                    .pb(px(8.))
                    .child(gen_btn)
                    .child(go),
            );
        div()
            .debug_selector(|| "obs-scm-foot".into())
            .flex_none()
            .pt(px(8.))
            .px(px(10.))
            .pb(px(10.))
            .border_t_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.pane))
            .child(compose)
            .into_any_element()
    }
}
