//! The Files panel (`.obs-files-panel`): the tree with sticky open folders, git letters,
//! finding and problem flags, unsaved dots, the inline rename / new field, the loading
//! skeleton, drag and drop, and the explorer status bar.
//!
//! Rows have fixed heights (a file `3 + 12.5 × 1.35 + 3`, a folder 24px, the rename field
//! 26px), so the panel lays itself out, paints only what is near the viewport, and draws open
//! folder headers in a layer at their sticky positions (GPUI has no `position: sticky`).

use recur_ui::guard::Guarded;
use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, AppContext as _, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Window, div, point, px,
};
use recur_domain::TreeEntry;
use recur_ui::exact::vgap;
use recur_ui::file_icons::file_icon;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::scroll_area;
use recur_ui::{IconName, Theme, icon};
use recur_view_core::file_drag::WorkspaceFileDrag;
use recur_view_core::workspace_nav::scroll_delta_to_reveal;
use recur_view_review::style::hsla;
use recur_view_review::tree_sticky::{StickyHeader, fade_hits_rows, sync_tree_sticky_fades};

use crate::explorer::{dir_status_index, visible_entries};
use crate::lsp::problems::{PathProblemFlag, ProblemSeverity, group_by_path};
use crate::obs::{DraftMode, Observatory};
use crate::style::{git_color, obs_empty, statusbar};

/// `--tree-row-h`.
pub const TREE_ROW_H: f32 = 24.;
/// `--tree-fade`.
pub const TREE_FADE: f32 = 20.;
/// `.obs-node` file row: padding 3px around a 12.5px × 1.35 line.
pub const FILE_ROW_H: f32 = 3. + 12.5 * 1.35 + 3.;
/// The rename / new field row: padding 3px around the 20px input.
pub const DRAFT_ROW_H: f32 = 26.;
/// `.obs-tree-empty`: padding 3px around 12px × 1.35.
pub const EMPTY_ROW_H: f32 = 3. + 12. * 1.35 + 3.;
/// Rows painted beyond the viewport on each side.
const OVERSCAN: f32 = 240.;

#[derive(Clone)]
enum RowKind {
    Node { open: bool, loading: bool },
    Draft,
    Empty,
}

#[derive(Clone)]
struct Row {
    entry: TreeEntry,
    kind: RowKind,
    depth: usize,
    top: f32,
    height: f32,
    branch_bottom: Option<f32>,
}

/// Drag ghost: the row's icon and name.
pub struct FileDragGhost {
    name: SharedString,
    dir: bool,
}

impl FileDragGhost {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(10.))
            .py(px(3.))
            .opacity(0.75)
            .bg(hsla(t.pane_2))
            .text_color(hsla(t.text))
            .when(!self.dir, |d| {
                d.child(
                    div()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(file_icon(self.name.clone())),
                )
            })
            .child(
                text(self.name.clone())
                    .size(12.5)
                    .line_height(LineHeight::Scale(1.35))
                    .nowrap(),
            )
    }
}

impl Render for FileDragGhost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}

/// The aggregate problem flag of a folder (`problemFlagFor` with `isDir`).
fn dir_problem_flag(path: &str, by: &BTreeMap<String, PathProblemFlag>) -> Option<PathProblemFlag> {
    let prefix = format!("{path}/");
    let (mut errors, mut warnings, mut count) = (0, 0, 0);
    let mut severity = ProblemSeverity::Info;
    for (p, f) in by {
        if p != path && !p.starts_with(&prefix) {
            continue;
        }
        errors += f.errors;
        warnings += f.warnings;
        count += f.count;
        if f.severity == ProblemSeverity::Error {
            severity = ProblemSeverity::Error;
        } else if f.severity == ProblemSeverity::Warning && severity != ProblemSeverity::Error {
            severity = ProblemSeverity::Warning;
        }
    }
    (count > 0).then_some(PathProblemFlag {
        errors,
        warnings,
        count,
        severity,
    })
}

impl Observatory {
    /// `visibleTree` and each folder's visible children, laid out with fixed heights.
    /// Rows the explorer lays out (on screen or culled).
    #[cfg(test)]
    pub(crate) fn explorer_row_count(&self, cx: &gpui_kit::App) -> usize {
        self.explorer_rows(cx).0.len()
    }

    fn explorer_rows(&self, cx: &gpui_kit::App) -> (Vec<Row>, f32) {
        let matchers = self.exclude_matchers(cx);
        let changed = self.filter_changed();
        let changed_tree;
        let roots: Vec<&TreeEntry> = if changed {
            changed_tree =
                recur_code::view::changed_tree::build_tree_from_paths(&self.review_paths(cx));
            changed_tree.iter().collect()
        } else {
            visible_entries(&self.tree, &matchers, None)
        };
        let mut rows = Vec::new();
        let mut y = 0.;
        let draft_new = self
            .draft_edit
            .as_ref()
            .filter(|d| d.mode != DraftMode::Rename)
            .map(|d| d.parent.clone());
        let renaming = self
            .draft_edit
            .as_ref()
            .filter(|d| d.mode == DraftMode::Rename)
            .map(|d| d.path.clone());
        #[allow(clippy::too_many_arguments)]
        fn walk(
            o: &Observatory,
            nodes: &[&TreeEntry],
            depth: usize,
            changed: bool,
            matchers: &[recur_code::glob::CompiledGlob],
            draft_new: &Option<String>,
            renaming: &Option<String>,
            rows: &mut Vec<Row>,
            y: &mut f32,
        ) {
            for e in nodes {
                let is_rename = renaming.as_deref() == Some(e.path.as_str());
                if e.dir {
                    let open = if changed {
                        !o.changed_collapsed.contains(&e.path)
                    } else {
                        o.expanded.contains(&e.path)
                    };
                    let loading = !changed && o.loading_dirs.contains(&e.path);
                    let at = rows.len();
                    let h = if is_rename { DRAFT_ROW_H } else { TREE_ROW_H };
                    rows.push(Row {
                        entry: (*e).clone(),
                        kind: if is_rename {
                            RowKind::Draft
                        } else {
                            RowKind::Node { open, loading }
                        },
                        depth,
                        top: *y,
                        height: h,
                        branch_bottom: None,
                    });
                    *y += h;
                    if open {
                        let kids: Vec<&TreeEntry> = if changed {
                            e.children.iter().collect()
                        } else {
                            visible_entries(&e.children, matchers, None)
                        };
                        let drafting_here = draft_new.as_deref() == Some(e.path.as_str());
                        if kids.is_empty() {
                            if o.loaded_dirs.contains(&e.path) && !loading && !drafting_here {
                                rows.push(Row {
                                    entry: (*e).clone(),
                                    kind: RowKind::Empty,
                                    depth,
                                    top: *y,
                                    height: EMPTY_ROW_H,
                                    branch_bottom: None,
                                });
                                *y += EMPTY_ROW_H;
                            }
                        } else {
                            walk(
                                o,
                                &kids,
                                depth + 1,
                                changed,
                                matchers,
                                draft_new,
                                renaming,
                                rows,
                                y,
                            );
                        }
                        if drafting_here {
                            rows.push(Row {
                                entry: TreeEntry {
                                    path: e.path.clone(),
                                    ..Default::default()
                                },
                                kind: RowKind::Draft,
                                depth: depth + 1,
                                top: *y,
                                height: DRAFT_ROW_H,
                                branch_bottom: None,
                            });
                            *y += DRAFT_ROW_H;
                        }
                        if !is_rename {
                            rows[at].branch_bottom = Some(*y);
                        }
                    }
                } else {
                    let h = if is_rename { DRAFT_ROW_H } else { FILE_ROW_H };
                    rows.push(Row {
                        entry: (*e).clone(),
                        kind: if is_rename {
                            RowKind::Draft
                        } else {
                            RowKind::Node {
                                open: false,
                                loading: false,
                            }
                        },
                        depth,
                        top: *y,
                        height: h,
                        branch_bottom: None,
                    });
                    *y += h;
                }
            }
        }
        walk(
            self, &roots, 0, changed, &matchers, &draft_new, &renaming, &mut rows, &mut y,
        );
        if draft_new.as_deref() == Some("") {
            rows.push(Row {
                entry: TreeEntry::default(),
                kind: RowKind::Draft,
                depth: 0,
                top: y,
                height: DRAFT_ROW_H,
                branch_bottom: None,
            });
            y += DRAFT_ROW_H;
        }
        (rows, y)
    }

    fn draft_row(&self, depth: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let Some(d) = &self.draft_edit else {
            return div().into_any_element();
        };
        let name = d.name(cx);
        let lead = if d.dir {
            div().size(px(12.)).flex_none().into_any_element()
        } else {
            div()
                .size(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(file_icon(SharedString::from(if name.is_empty() {
                    "file".to_string()
                } else {
                    name
                })))
                .into_any_element()
        };
        let field = div()
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h(px(20.))
            .px(px(4.))
            .border_1()
            .border_color(hsla(t.amber))
            .rounded(px(3.))
            .bg(hsla(t.bg))
            .child(recur_ui::style::text_field(
                &d.input,
                &SharedString::default(),
                recur_ui::style::UA_PLACEHOLDER,
                recur_ui::fonts::Family::Ui,
                12.5,
                window,
                cx,
            ));
        div()
            .debug_selector(|| "obs-node-draft".into())
            .flex()
            .items_center()
            .gap(px(4.))
            .w_full()
            .h(px(DRAFT_ROW_H))
            .pl(px(10. + depth as f32 * 12.))
            .pr(px(10.))
            .capture_key_down(cx.guarded_listener(|o, e: &gpui_kit::KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.stop_propagation();
                    o.cancel_draft(cx);
                }
            }))
            .child(lead)
            .child(div().flex_1().min_w_0().h(px(20.)).child(field))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn node_row(
        &self,
        row: &Row,
        dir_index: &BTreeMap<String, String>,
        finding_flags: &std::collections::HashMap<String, recur_view_review::findings::FindingFlag>,
        problems: &BTreeMap<String, PathProblemFlag>,
        unsaved: &BTreeSet<String>,
        sticky: Option<bool>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let e = &row.entry;
        let RowKind::Node { open, loading } = row.kind else {
            return div().into_any_element();
        };
        let st = self.entry_status(e, dir_index);
        let on = self.tree_path == e.path;
        let hovered = self.hovered_row.as_deref() == Some(e.path.as_str());
        let cut = self
            .file_clip
            .as_ref()
            .is_some_and(|c| c.cut && c.path == e.path);
        let deleted = st == "d" && !e.dir;
        let drop_into = e.dir && self.tree_drop_dir.as_deref() == Some(e.path.as_str());
        let bg = if hovered || on {
            Some(t.pane_2)
        } else if e.dir {
            Some(t.bg)
        } else {
            None
        };
        let fg = if e.ignored { t.muted } else { t.text };
        let lead = if e.dir {
            if loading {
                div()
                    .size(px(12.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        text("…")
                            .size(11.)
                            .line_height(LineHeight::Scale(1.))
                            .color(t.faint),
                    )
                    .into_any_element()
            } else {
                icon(IconName::ChevronDown)
                    .size(px(12.))
                    .stroke_width(2.4)
                    .color(hsla(t.faint))
                    .rotate(if open { 0. } else { -90. })
                    .into_any_element()
            }
        } else {
            div()
                .size(px(14.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(file_icon(SharedString::from(e.name.clone())))
                .into_any_element()
        };
        let mut label = text(e.name.clone())
            .size(12.5)
            .line_height(LineHeight::Scale(1.35))
            .truncate();
        if let Some(c) = git_color(&st, &t) {
            label = label.color(c);
        }
        if deleted {
            label = label.line_through(true);
        }
        let label = div()
            .min_w_0()
            .when(deleted, |d| d.opacity(0.75))
            .child(label);
        let mut label_row = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .flex_1()
            .child(label);
        if !e.dir && unsaved.contains(&e.path) {
            let p = e.path.clone();
            label_row = label_row.child(
                div()
                    .debug_selector(move || format!("obs-unsaved:{p}"))
                    .size(px(5.))
                    .rounded(px(2.5))
                    .flex_none()
                    .bg(hsla(t.amber)),
            );
        }
        let mut d = div()
            .id(SharedString::from(format!("node-{}", e.path)))
            .debug_selector({
                let p = e.path.clone();
                let dir = e.dir;
                move || format!("obs-node:{}:{p}", if dir { "dir" } else { "file" })
            })
            .relative()
            .flex()
            .items_center()
            .gap(px(4.))
            .w_full()
            .pl(px(10. + row.depth as f32 * 12.))
            .pr(px(10.))
            .when(e.ignored, |d| {
                let p = e.path.clone();
                d.child(
                    div()
                        .absolute()
                        .size_0()
                        .debug_selector(move || format!("obs-ignored:{p}")),
                )
            })
            .text_color(hsla(fg))
            .when_some(bg, |d, c| d.bg(hsla(c)))
            .when(e.dir, |d| d.h(px(TREE_ROW_H)))
            .when(!e.dir, |d| d.py(px(3.)))
            .when(cut, |d| d.opacity(0.45))
            .child(lead)
            .child(label_row);
        let flag = recur_view_review::findings::finding_flag_for_path(
            &e.path,
            e.dir,
            &finding_flags
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
        );
        if flag.count > 0 {
            let c = match flag.severity.as_str() {
                "high" | "critical" => t.coral,
                "low" | "info" => t.muted,
                _ => t.amber,
            };
            let label = format!(
                "{} open finding{}",
                flag.count,
                if flag.count == 1 { "" } else { "s" }
            );
            d = d.child(titled(
                SharedString::from(format!("nff-t-{}", e.path)),
                label,
                div()
                    .id(SharedString::from(format!("nff-{}", e.path)))
                    .debug_selector({
                        let p = e.path.clone();
                        move || format!("obs-finding-flag:{p}")
                    })
                    .flex_none()
                    .size(px(12.))
                    .ml(px(2.))
                    .when(e.dir, |d| d.opacity(0.55))
                    .child(
                        icon(IconName::HatGlasses)
                            .size(px(12.))
                            .stroke_width(2.2)
                            .color(hsla(c)),
                    ),
            ));
        }
        let pflag = if e.dir {
            dir_problem_flag(&e.path, problems)
        } else {
            problems.get(&e.path).cloned()
        };
        if let Some(pf) = pflag {
            let (fg, bg) = match pf.severity {
                ProblemSeverity::Warning => (t.amber, t.amber.fade(0.16)),
                ProblemSeverity::Info | ProblemSeverity::Hint => (t.muted, t.pane_2),
                ProblemSeverity::Error => (t.coral, t.coral.fade(0.16)),
            };
            let n = if pf.errors > 0 {
                pf.errors
            } else if pf.warnings > 0 {
                pf.warnings
            } else {
                pf.count
            };
            let title = format!(
                "{} error{}, {} warning{}",
                pf.errors,
                if pf.errors == 1 { "" } else { "s" },
                pf.warnings,
                if pf.warnings == 1 { "" } else { "s" }
            );
            d = d.child(titled(
                SharedString::from(format!("npf-t-{}", e.path)),
                title,
                div()
                    .id(SharedString::from(format!("npf-{}", e.path)))
                    .debug_selector({
                        let p = e.path.clone();
                        move || format!("obs-problem-flag:{p}")
                    })
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .min_w(px(14.))
                    .h(px(14.))
                    .ml(px(2.))
                    .px(px(3.))
                    .rounded(px(4.))
                    .bg(hsla(bg))
                    .when(e.dir, |d| d.opacity(0.6))
                    .child(
                        text(n.to_string())
                            .mono()
                            .size(9.)
                            .weight(700.)
                            .line_height(LineHeight::Scale(1.))
                            .color(fg)
                            .nowrap(),
                    ),
            ));
        }
        if !st.is_empty() && !e.dir {
            let wip = self.working_status.contains_key(&e.path);
            let title = match st.as_str() {
                "m" => "Modified",
                "a" => "Added",
                "d" => "Deleted",
                "u" => "Untracked",
                other => other,
            };
            let c = match st.as_str() {
                "m" => t.git_m,
                "a" | "u" => t.git_u,
                "d" => t.git_d,
                _ => t.text,
            };
            d = d.child(titled(
                SharedString::from(format!("nst-t-{}", e.path)),
                title.to_string(),
                div()
                    .id(SharedString::from(format!("nst-{}", e.path)))
                    .debug_selector({
                        let p = e.path.clone();
                        let st = st.clone();
                        move || format!("obs-fst:{st}:{p}")
                    })
                    .relative()
                    .w(px(12.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(
                        text(st.to_uppercase())
                            .size(10.)
                            .weight(700.)
                            .tracking(0.02)
                            .line_height(LineHeight::Scale(1.35))
                            .color(c)
                            .nowrap(),
                    )
                    .when(wip, |d| {
                        d.child(
                            div()
                                .absolute()
                                .left(px(4.5))
                                .bottom(px(-2.))
                                .size(px(3.))
                                .rounded(px(1.5))
                                .bg(hsla(t.amber)),
                        )
                    }),
            ));
        }
        let entry = e.clone();
        let entry2 = e.clone();
        let entry3 = e.clone();
        let path = e.path.clone();
        let drag_payload = if e.dir {
            format!("{}/", e.path.trim_end_matches('/'))
        } else {
            e.path.clone()
        };
        let ghost_name = SharedString::from(e.name.clone());
        let ghost_dir = e.dir;
        let d = d
            .on_hover(cx.guarded_listener(move |o, on: &bool, _, cx| {
                if *on {
                    o.hovered_row = Some(path.clone());
                } else if o.hovered_row.as_deref() == Some(path.as_str()) {
                    o.hovered_row = None;
                }
                cx.notify();
            }))
            .on_click(cx.guarded_listener(move |o, ev: &ClickEvent, window, cx| {
                o.on_node_click(&entry, ev.click_count(), window, cx);
                window.focus(&o.files_focus, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.guarded_listener(move |o, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    o.open_ctx(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        &entry2.path,
                        entry2.dir,
                        &entry2.name,
                        false,
                        window,
                        cx,
                    );
                }),
            )
            .on_drag(
                WorkspaceFileDrag::new(&[drag_payload]),
                move |_, _, _, cx| {
                    cx.new(|_| FileDragGhost {
                        name: ghost_name.clone(),
                        dir: ghost_dir,
                    })
                },
            )
            .drag_over::<WorkspaceFileDrag>(move |s, _, _, _| s)
            .on_drop(
                cx.guarded_listener(move |o, drag: &WorkspaceFileDrag, window, cx| {
                    let Some(dir) = o.tree_drop_target(Some((&entry3.path, entry3.dir))) else {
                        return;
                    };
                    o.on_tree_drop(dir, drag.paths.clone(), window, cx);
                }),
            );
        let d = if drop_into {
            div()
                .relative()
                .child(d)
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_1()
                        .border_color(hsla(t.line)),
                )
                .into_any_element()
        } else {
            d.into_any_element()
        };
        let _ = sticky;
        d
    }

    /// The `.obs-files` scroller with its rows and sticky header layer.
    pub(crate) fn render_files_tree(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let (rows, content_h) = self.explorer_rows(cx);
        let dir_index = dir_status_index(&self.git_status);
        let finding_flags = self.open_findings_by_path(cx);
        let problems = group_by_path(&self.problems(cx));
        let unsaved: BTreeSet<String> = self.unsaved_paths().into_iter().collect();
        let view = self.files_scroll.bounds();
        let view_h = f32::from(view.size.height);
        let max_scroll = (content_h - view_h).max(0.);
        let scroll = (-f32::from(self.files_scroll.offset().y)).clamp(0., max_scroll);
        if let Some(path) = self.pending_explorer_scroll.take() {
            if view_h > 0. {
                if let Some(r) = rows
                    .iter()
                    .find(|r| r.entry.path == path && matches!(r.kind, RowKind::Node { .. }))
                {
                    let stuck = r.depth as f32 * TREE_ROW_H;
                    let delta = scroll_delta_to_reveal(
                        r.top - scroll,
                        r.top - scroll + r.height,
                        0.,
                        view_h,
                        stuck,
                        0.,
                    );
                    if delta != 0. {
                        let next = (scroll + delta).clamp(0., max_scroll);
                        self.files_scroll.set_offset(point(px(0.), px(-next)));
                        cx.notify();
                    }
                }
            } else {
                self.pending_explorer_scroll = Some(path);
                cx.notify();
            }
        }
        let open_dirs: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| {
                matches!(r.kind, RowKind::Node { open: true, .. })
                    && r.entry.dir
                    && r.branch_bottom.is_some()
            })
            .map(|(i, _)| i)
            .collect();
        let tops: Vec<f32> = open_dirs
            .iter()
            .map(|&i| {
                let r = &rows[i];
                (scroll + r.depth as f32 * TREE_ROW_H)
                    .min(r.branch_bottom.unwrap_or(r.top) - TREE_ROW_H)
                    .max(r.top)
            })
            .collect();
        let headers: Vec<StickyHeader> = open_dirs
            .iter()
            .zip(&tops)
            .map(|(&i, &top)| StickyHeader {
                depth: rows[i].depth as f32,
                top: top - scroll,
            })
            .collect();
        let live: Vec<(f32, f32)> = rows
            .iter()
            .filter(|r| {
                self.tree_path == r.entry.path
                    || self.hovered_row.as_deref() == Some(r.entry.path.as_str())
            })
            .map(|r| (r.top - scroll, r.top - scroll + r.height))
            .collect();
        let sticky = sync_tree_sticky_fades(scroll, 0., &headers, TREE_ROW_H, |i| {
            let h = &headers[i];
            let own = (
                rows[open_dirs[i]].top - scroll,
                rows[open_dirs[i]].top - scroll + TREE_ROW_H,
            );
            let others: Vec<(f32, f32)> = live.iter().copied().filter(|r| *r != own).collect();
            fade_hits_rows(h.top + TREE_ROW_H, TREE_FADE, &others)
        });
        let lo = scroll - OVERSCAN;
        let hi = scroll + view_h.max(600.) + OVERSCAN;
        let mut flow = div().relative().flex().flex_col().w_full();
        let mut gap = 0.;
        for (i, r) in rows.iter().enumerate() {
            let visible = r.top + r.height >= lo && r.top <= hi;
            if !visible || open_dirs.contains(&i) {
                gap += r.height;
                continue;
            }
            if gap > 0. {
                flow = flow.child(vgap(gap, window));
                gap = 0.;
            }
            let el = match r.kind {
                RowKind::Draft => self.draft_row(r.depth, window, cx),
                RowKind::Empty => div()
                    .pl(px(22. + r.depth as f32 * 12.))
                    .pr(px(10.))
                    .py(px(3.))
                    .child(
                        text("empty")
                            .size(12.)
                            .line_height(LineHeight::Scale(1.35))
                            .color(t.faint),
                    )
                    .into_any_element(),
                RowKind::Node { .. } => {
                    self.node_row(r, &dir_index, &finding_flags, &problems, &unsaved, None, cx)
                }
            };
            flow = flow.child(el);
        }
        if gap > 0. {
            flow = flow.child(vgap(gap, window));
        }
        let mut order: Vec<usize> = (0..open_dirs.len()).filter(|&k| !sticky[k].0).collect();
        let mut stuck: Vec<usize> = (0..open_dirs.len()).filter(|&k| sticky[k].0).collect();
        stuck.sort_by(|a, b| rows[open_dirs[*b]].depth.cmp(&rows[open_dirs[*a]].depth));
        order.extend(stuck);
        let mut layer: Vec<AnyElement> = Vec::new();
        for k in order {
            let r = &rows[open_dirs[k]];
            let top = tops[k];
            if top + TREE_ROW_H + TREE_FADE < lo || top > hi {
                continue;
            }
            let (is_stuck, fade_under) = sticky[k];
            let row_el = self.node_row(
                r,
                &dir_index,
                &finding_flags,
                &problems,
                &unsaved,
                Some(is_stuck),
                cx,
            );
            let hovered = self.hovered_row.as_deref() == Some(r.entry.path.as_str())
                || self.tree_path == r.entry.path;
            let fade_from = if hovered { t.pane_2 } else { t.bg };
            let fade_path = r.entry.path.clone();
            let fade = fade_under.then(|| {
                div()
                    .debug_selector(move || format!("obs-node-fade:{fade_path}"))
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(TREE_ROW_H))
                    .h(px(TREE_FADE))
                    .child(recur_ui::style::gradient_fill(
                        180.,
                        (fade_from, 0.),
                        (fade_from.with_alpha(0.), 1.),
                    ))
            });
            layer.push(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .flex_col()
                    .child(vgap(top, window))
                    .child({
                        let p = r.entry.path.clone();
                        div()
                            .relative()
                            .when(is_stuck, |d| {
                                d.debug_selector(move || format!("obs-stuck:{p}"))
                            })
                            .child(row_el)
                            .children(fade)
                    })
                    .into_any_element(),
            );
        }
        flow = flow.children(layer);
        if self.show_tree_skeleton && rows.is_empty() {
            let mut sk = div().py(px(2.)).flex().flex_col();
            for (i, w) in [70., 52., 61., 44., 58., 48.].iter().enumerate() {
                let pad = if i == 0 || i == 3 { 10. } else { 22. };
                sk = sk.child(
                    div()
                        .debug_selector(move || format!("obs-skel-row:{i}"))
                        .flex()
                        .items_center()
                        .h(px(22.))
                        .pl(px(pad))
                        .pr(px(10.))
                        .child(
                            div()
                                .w(gpui_kit::relative(w / 100.))
                                .h(px(9.))
                                .child(recur_ui::style::skeleton(window, cx).size_full()),
                        ),
                );
            }
            flow = flow.child(sk);
        }
        if !self.tree_loading && rows.is_empty() && self.draft_edit.is_none() {
            let msg = if self.worktree_missing {
                "This workspace's folder no longer exists."
            } else if self.filter_changed() {
                "No changes since base."
            } else {
                "No files — set a repo path on the project."
            };
            flow = flow.child(obs_empty(msg, false, cx));
        } else if self.worktree_missing {
            flow = flow.child(obs_empty(
                "This workspace's folder no longer exists.",
                false,
                cx,
            ));
        }
        let focused_ring = recur_ui::style::is_focus_visible(&self.files_focus, window);
        let root_drop = self.tree_drop_dir.as_deref() == Some("");
        div()
            .id("obs-files")
            .debug_selector(|| "obs-files".into())
            .track_focus(&self.files_focus)
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(crate::geometry::probe(&self.bounds.files))
            .child(
                scroll_area("obs-files-scroll")
                    .overlay()
                    .track(&self.files_scroll)
                    .child(flow),
            )
            .when(focused_ring || root_drop, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_1()
                        .border_color(hsla(if focused_ring { t.line_bright } else { t.line })),
                )
            })
            .on_key_down(
                cx.guarded_listener(|o, e: &gpui_kit::KeyDownEvent, window, cx| {
                    let k = &e.keystroke;
                    let m = k.modifiers;
                    let typing = o.draft_edit.is_some();
                    if o.on_files_key(&k.key, m.secondary(), m.alt, m.shift, typing, window, cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.guarded_listener(|o, ev: &MouseDownEvent, window, cx| {
                    o.open_ctx(
                        f32::from(ev.position.x),
                        f32::from(ev.position.y),
                        "",
                        true,
                        "",
                        false,
                        window,
                        cx,
                    );
                }),
            )
            .on_drop(
                cx.guarded_listener(|o, drag: &WorkspaceFileDrag, window, cx| {
                    if let Some(dir) = o.tree_drop_target(None) {
                        o.on_tree_drop(dir, drag.paths.clone(), window, cx);
                    }
                }),
            )
            .into_any_element()
    }

    /// `.obs-files-panel`: the tree over the explorer status bar.
    pub(crate) fn render_files_panel(
        &mut self,
        in_side: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let tree = self.render_files_tree(window, cx);
        let mut bar = statusbar(cx).px(px(6.)).gap(px(2.));
        if in_side {
            bar = bar.pr(px(crate::style::WIN_CORNER_PAD));
        } else if self.state.read(cx).rail_collapsed {
            bar = bar.pl(px(crate::style::WIN_CORNER_PAD));
        }
        if self.tree_writable() {
            let files_btn = |id: &'static str,
                             name: IconName,
                             tip: &'static str,
                             folder: bool,
                             cx: &mut Context<Self>| {
                titled(
                    SharedString::from(format!("{id}-t")),
                    tip,
                    div()
                        .id(id)
                        .debug_selector(move || id.into())
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .size(px(22.))
                        .rounded(px(5.))
                        .cursor_pointer()
                        .text_color(hsla(t.muted))
                        .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                        .child(icon(name).size(px(14.)))
                        .on_click(cx.guarded_listener(move |o, _, window, cx| {
                            let dir = o.action_dir(cx);
                            o.start_new(folder, &dir, window, cx);
                        })),
                )
            };
            bar = bar
                .child(files_btn(
                    "obs-new-file",
                    IconName::FilePlus,
                    "New File (⌥⌘N)",
                    false,
                    cx,
                ))
                .child(files_btn(
                    "obs-new-folder",
                    IconName::FolderPlus,
                    "New Folder (⇧⌘N)",
                    true,
                    cx,
                ));
        } else {
            bar = bar.child(crate::style::foot_text("Browse only"));
        }
        bar = bar.child(div().flex_1());
        let dirty = self.working_dirty_count();
        let findings = self.open_findings_count();
        if dirty > 0 {
            bar = bar.child(
                crate::style::linkish("obs-dirty-link", Some(t.amber), cx)
                    .debug_selector(|| "obs-dirty-link".into())
                    .child(crate::style::link_text(format!("{dirty} dirty")).weight(600.))
                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                        o.select_code_panel(recur_view_core::prefs::CodePanelKey::Scm, window, cx)
                    })),
            );
        }
        if dirty > 0 && findings > 0 {
            bar = bar.child(div().mx(px(4.)).child(crate::style::foot_sep(cx)));
        }
        if findings > 0 {
            bar = bar.child(
                crate::style::linkish("obs-findings-link", Some(t.amber), cx)
                    .child(
                        crate::style::link_text(format!(
                            "{findings} finding{}",
                            if findings == 1 { "" } else { "s" }
                        ))
                        .weight(600.),
                    )
                    .on_click(cx.guarded_listener(|o, _, window, cx| {
                        o.select_code_panel(
                            recur_view_core::prefs::CodePanelKey::Findings,
                            window,
                            cx,
                        )
                    })),
            );
        }
        div()
            .debug_selector(|| "obs-files-panel".into())
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .h_full()
            .child(tree)
            .child(bar)
            .into_any_element()
    }
}
