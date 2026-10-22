//! The Files changed pane: the toolbar (file list toggle and mode, counts, totals, unified /
//! split, fold all, Post all), the file list with its filter and the findings / comments
//! toggles, and the stacked diffs.

use std::collections::HashSet;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, px,
};
use recur_code::view::changed_tree::sort_files_by_changed_tree;
use recur_code::view::finding_paths::{findings_for_path, match_finding_path};
use recur_domain::{FileDiff, Finding, Pr};
use recur_engine::view::comment_thread::comments_for_path;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::{Axis, Edge, resize_handle};
use recur_ui::{IconName, Theme, icon};
use recur_view_core::prefs::PanelKey;
use recur_view_review::diff_file::DiffFileHooks;
use recur_view_review::file_list::FileListProps;
use recur_view_review::findings::FindingsByPath;
use recur_view_review::review_diff::{DiffLayout, FileListMode};
use recur_view_review::stack::CardProps;
use recur_view_review::style::{icon_btn, note_btn, seg, seg_btn};

use super::PrDetail;
use crate::style::hsla;

/// Side-by-side needs this much pane width.
const SPLIT_MIN_PX: f32 = 720.;

impl PrDetail {
    fn filter_q(&self, cx: &App) -> String {
        self.filter.read(cx).value().trim().to_lowercase()
    }

    /// `findingsByPath`: listed findings keyed by their PR file.
    pub(crate) fn findings_by_path(&self, cx: &App) -> FindingsByPath {
        let paths: Vec<String> = self.review_files(cx).into_iter().map(|f| f.path).collect();
        let mut out: FindingsByPath = Vec::new();
        for f in self.listed_findings(cx) {
            if f.file.is_empty() {
                continue;
            }
            let m = match_finding_path(&f.file, &paths);
            let p = if m.is_empty() {
                f.file.clone()
            } else {
                m.to_string()
            };
            match out.iter_mut().find(|(k, _)| *k == p) {
                Some((_, v)) => v.push(f),
                None => out.push((p, vec![f])),
            }
        }
        out
    }

    /// `matchedFiles`.
    fn matched_files(&self, cx: &App) -> Vec<FileDiff> {
        let q = self.filter_q(cx);
        let mut list: Vec<FileDiff> = self
            .review_files(cx)
            .into_iter()
            .filter(|f| {
                q.is_empty()
                    || f.path.to_lowercase().contains(&q)
                    || f.old_path.to_lowercase().contains(&q)
            })
            .collect();
        if self.findings_only {
            let by = self.findings_by_path(cx);
            list.retain(|f| !findings_for_path(&by, &f.path).is_empty());
        }
        if self.comments_only {
            let by = self.comments_by_path(cx);
            list.retain(|f| !comments_for_path(&by, &f.path).is_empty());
        }
        sort_files_by_changed_tree(&list, |f| f.path.as_str())
    }

    pub(crate) fn split_fallback(&self) -> bool {
        self.diff_layout == DiffLayout::Split
            && self.pane_width > 0.
            && self.pane_width < SPLIT_MIN_PX
    }

    /// `diffLayoutSeg`.
    pub(crate) fn diff_layout_seg(&self, _in_files: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let layout = self.diff_layout;
        let fallback = self.split_fallback();
        seg(
            [
                titled(
                    "pr-dl-unified-t",
                    "Unified",
                    seg_btn(
                        "pr-dl-unified",
                        IconName::Rows2,
                        layout == DiffLayout::Unified,
                        None,
                        cx,
                    )
                    .debug_selector(move || {
                        format!("button:Unified diff:{}", layout == DiffLayout::Unified)
                    })
                    .on_click(
                        cx.listener(|d, _, _, cx| d.set_diff_layout(DiffLayout::Unified, cx)),
                    ),
                )
                .into_any_element(),
                titled(
                    "pr-dl-split-t",
                    if fallback {
                        "Too narrow for side-by-side — widen the pane or hide the file list"
                    } else {
                        "Side by side"
                    },
                    seg_btn(
                        "pr-dl-split",
                        IconName::Columns2,
                        layout == DiffLayout::Split,
                        fallback.then_some(t.pane),
                        cx,
                    )
                    .debug_selector(move || {
                        format!("button:Side-by-side diff:{}", layout == DiffLayout::Split)
                    })
                    .on_click(cx.listener(move |d, _, _, cx| {
                        if !fallback {
                            d.set_diff_layout(DiffLayout::Split, cx)
                        }
                    })),
                )
                .into_any_element(),
            ],
            cx,
        )
        .into_any_element()
    }

    /// The Files changed pane.
    pub(crate) fn files_pane(
        &mut self,
        pr: Option<&Pr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let Some(pr) = pr else {
            return div().flex_1().into_any_element();
        };
        if self.files_pending(cx) {
            return div()
                .debug_selector(|| "prpane:files".into())
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(hsla(t.bg))
                .child(super::skeleton::files_skel(window, cx))
                .into_any_element();
        }
        let review = self.review_files(cx);
        let matched = self.matched_files(cx);
        let q = self.filter_q(cx);
        let filtering = !q.is_empty() || self.findings_only || self.comments_only;
        let add: i64 = matched.iter().map(|f| f.diff.add).sum();
        let del: i64 = matched.iter().map(|f| f.diff.del).sum();
        let listed = self.listed_findings(cx);
        let findings_by = Rc::new(self.findings_by_path(cx));
        let comments_by = self.comments_by_path(cx);
        let can_post = self.can_post_findings(cx);
        let postable = self.postable_findings(cx);
        let postable_ids: HashSet<String> = postable.iter().map(|f| f.id.clone()).collect();
        let list_w = self.state().read(cx).panels.size(PanelKey::PrFileList);
        let open = self.sidebar_open;
        let lh = recur_ui::style::button_label("x", 11., 400.)
            .measure(None, window)
            .2
            .height;

        // The stack and the list follow the inputs each frame.
        let files: Vec<Rc<FileDiff>> = matched.iter().cloned().map(Rc::new).collect();
        self.stack.set_files(files.clone());
        let state = self.state().clone();
        let ws = pr.workspace_id.clone();
        let mut hooks = DiffFileHooks::default();
        if !ws.is_empty() {
            let st = state.clone();
            hooks.open_file = Some(Rc::new(move |p, l, _, cx| {
                PrDetail::open_in_workspace(&st, &p, l, cx)
            }));
            let me = cx.entity().downgrade();
            hooks.open_finding = Some(Rc::new(move |f: Finding, _, cx| {
                let _ = me.update(cx, |d, cx| d.open_finding(&f, cx));
            }));
        }
        if can_post {
            let me = cx.entity().downgrade();
            hooks.post_finding = Some(Rc::new(move |f: Finding, _, cx| {
                let _ = me.update(cx, |d, cx| d.post_findings(vec![f], cx));
            }));
        }
        let layout = if self.split_fallback() {
            DiffLayout::Unified
        } else {
            self.diff_layout
        };
        self.stack.set_props(CardProps {
            layout,
            note_mode: None,
            comments: true,
            collapsible: true,
            open_file_title: Some("Open in workspace".into()),
            findings_by_path: Some(findings_by.clone()),
            highlight_line: self.highlight,
            posting_finding_ids: self.posting.clone(),
            postable_finding_ids: postable_ids,
            hooks,
        });
        let props = FileListProps {
            files: files.clone(),
            selected_path: self.stack.selected().to_string(),
            mode: self.file_list_mode,
            findings_by_path: Some(findings_by),
            comments_by_path: Some(Rc::new(comments_by.clone())),
            dirty_paths: None,
            reveal: self.stack.toc_reveal,
        };
        self.list.update(cx, |l, cx| l.set_props(props, cx));

        // `.fl-head`
        let toggle_label = if open {
            "Hide file list"
        } else {
            "Show file list"
        };
        let toggle = titled(
            "pr-fl-toggle-t",
            toggle_label,
            div()
                .id("pr-fl-toggle")
                .debug_selector(move || format!("button:{toggle_label}"))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(28.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(hsla(if open { t.text } else { t.faint }))
                .hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
                .child(icon(IconName::PanelLeft).size(px(16.)))
                .on_click(cx.listener(move |d, _, _, cx| d.set_sidebar_open(!open, cx))),
        );
        let mut cluster = div()
            .flex()
            .items_center()
            .h_full()
            .min_w_0()
            .px(px(8.))
            .border_r_1()
            .border_color(hsla(if open {
                t.line
            } else {
                recur_ui::color::TRANSPARENT
            }))
            .child(toggle);
        if open {
            let mode = self.file_list_mode;
            cluster = cluster.child(
                div().ml_auto().child(seg(
                    [
                        titled(
                            "pr-fl-flat-t",
                            "Flat list",
                            seg_btn(
                                "pr-fl-flat",
                                IconName::List,
                                mode == FileListMode::Flat,
                                None,
                                cx,
                            )
                            .debug_selector(move || {
                                format!("button:Flat list:{}", mode == FileListMode::Flat)
                            })
                            .on_click(cx.listener(|d, _, _, cx| {
                                d.set_file_list_mode(FileListMode::Flat, cx)
                            })),
                        )
                        .into_any_element(),
                        titled(
                            "pr-fl-tree-t",
                            "Tree",
                            seg_btn(
                                "pr-fl-tree",
                                IconName::Workflow,
                                mode == FileListMode::Tree,
                                None,
                                cx,
                            )
                            .debug_selector(move || {
                                format!("button:Tree:{}", mode == FileListMode::Tree)
                            })
                            .on_click(cx.listener(|d, _, _, cx| {
                                d.set_file_list_mode(FileListMode::Tree, cx)
                            })),
                        )
                        .into_any_element(),
                    ],
                    cx,
                )),
            );
        }
        let total = pr.files.as_ref().map_or(0, Vec::len);
        let count = if filtering {
            format!("{}/{} files", matched.len(), review.len())
        } else {
            format!("{}/{} files", review.len(), total)
        };
        let mut main = div()
            .flex()
            .items_center()
            .min_w_0()
            .h_full()
            .px(px(13.))
            .child(
                text(count.clone())
                    .mono()
                    .size(9.5)
                    .tracking(0.1)
                    .uppercase()
                    .color(t.faint)
                    .nowrap()
                    .id("fl-head-count"),
            )
            .debug_selector(move || format!("fl-head-count={count}"));
        main = main.child(div().ml_auto().child(recur_view_review::style::add_del(
            add,
            del,
            Family::Mono,
            9.5,
            5.,
            LineHeight::Scale(1.45),
            cx,
        )));
        main = main.child(div().ml(px(8.)).child(self.diff_layout_seg(true, cx)));
        let any = self.stack.any_expanded();
        let fold_label = if any {
            "Collapse all files"
        } else {
            "Expand all files"
        };
        main = main.child(
            div().ml(px(4.)).child(titled(
                "pr-fold-all-t",
                fold_label,
                icon_btn(
                    "pr-fold-all",
                    if any {
                        IconName::FoldVertical
                    } else {
                        IconName::UnfoldVertical
                    },
                    26.,
                    15.,
                    None,
                    cx,
                )
                .debug_selector(move || format!("button:{fold_label}"))
                .on_click(cx.listener(|d, _, _, cx| {
                    d.stack.toggle_fold_all();
                    cx.notify();
                })),
            )),
        );
        if can_post && !postable.is_empty() {
            let busy = postable.iter().any(|f| self.posting.contains(&f.id));
            let list = postable.clone();
            main = main.child(
                div().ml(px(8.)).child(
                    note_btn("pr-fl-post-all", "Post all", false, busy, t.pane, lh, cx)
                        .debug_selector(|| "fl-head:Post all".into())
                        .when(!busy, |b| {
                            b.on_click(
                                cx.listener(move |d, _, _, cx| d.post_findings(list.clone(), cx)),
                            )
                        }),
                ),
            );
        }
        let head = div()
            .debug_selector(|| "fl-head".into())
            .flex()
            .items_center()
            .flex_none()
            .h(px(40.))
            .bg(hsla(t.pane))
            .border_b_1()
            .border_color(hsla(t.line))
            .overflow_hidden()
            .child(if open {
                div().flex_none().w(px(list_w)).h_full().child(cluster)
            } else {
                div().flex_none().h_full().child(cluster)
            })
            .child(div().flex_1().min_w_0().h_full().child(main));

        // `.diffwrap`
        let mut wrap = div()
            .debug_selector(|| "diffwrap".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex();
        if open {
            wrap = wrap.child(self.file_list_panel(
                list_w,
                !listed.is_empty(),
                !comments_by.is_empty(),
                window,
                cx,
            ));
        }
        let empty = (filtering && matched.is_empty()).then(|| {
            recur_view_review::commits::empty_clear_filter(
                cx.listener(|d, _, window, cx| {
                    d.filter.update(cx, |f, cx| f.set_value("", window, cx));
                    d.findings_only = false;
                    d.comments_only = false;
                    cx.notify();
                }),
                cx,
            )
        });
        let stack = self.stack.render(
            Vec::new(),
            empty,
            |d: &mut PrDetail| &mut d.stack,
            Rc::new(|d: &mut PrDetail, path, next, cx| {
                d.stack.toggle_collapsed(&path, next);
                cx.notify();
            }),
            window,
            cx,
        );
        let probe = {
            let me = cx.entity().downgrade();
            let pane = self.stack.pane.clone();
            canvas(
                move |b, _, cx| {
                    let bar = if pane.max_scroll() > 0.5 {
                        f32::from(recur_ui::theme::SCROLLBAR_SIZE)
                    } else {
                        0.
                    };
                    let w = f32::from(b.size.width) - bar;
                    let me = me.clone();
                    cx.defer(move |cx| {
                        let _ = me.update(cx, |d, cx| {
                            if (d.pane_width - w).abs() > 0.5 {
                                d.pane_width = w;
                                cx.notify();
                            }
                        });
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        wrap = wrap.child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .child(stack)
                .child(probe),
        );
        div()
            .debug_selector(|| "prpane:files".into())
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(hsla(t.bg))
            .child(head)
            .child(wrap)
            .into_any_element()
    }

    /// `.filelist`: the resize handle, the filter row and the list.
    fn file_list_panel(
        &mut self,
        width: f32,
        has_findings: bool,
        has_comments: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let state = self.state().clone();
        let focus = cx.focus_handle();
        let handle = resize_handle("pr-fl-resize", Axis::X, Edge::End, width, &focus)
            .on_live({
                let state = state.clone();
                move |w, _, cx| {
                    state.update(cx, |s, cx| s.set_panel_live(PanelKey::PrFileList, w, cx))
                }
            })
            .on_commit({
                let state = state.clone();
                move |_, cx| state.update(cx, |s, _| s.persist_panels())
            })
            .on_step(move |w, _, cx| {
                state.update(cx, |s, cx| s.set_panel(PanelKey::PrFileList, w, cx))
            });
        let mut filter = div()
            .debug_selector(|| "fl-filter".into())
            .flex()
            .items_center()
            .gap(px(6.))
            .flex_none()
            .py(px(6.))
            .px(px(8.))
            .border_b_1()
            .border_color(hsla(t.line))
            .text_color(hsla(t.faint))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_1()
                    .min_w_0()
                    .key_context("TextEntry")
                    .on_key_down(cx.listener(|d, ev: &KeyDownEvent, window, cx| {
                        if ev.keystroke.key == "escape" {
                            cx.stop_propagation();
                            d.filter.update(cx, |f, cx| f.set_value("", window, cx));
                            cx.notify();
                        }
                    }))
                    .child(icon(IconName::Search).size(px(12.)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(19.))
                            .flex()
                            .items_center()
                            .child(recur_ui::style::text_field(
                                &self.filter,
                                &SharedString::from("Filter files"),
                                t.faint,
                                Family::Ui,
                                12.,
                                window,
                                cx,
                            )),
                    ),
            );
        if has_findings {
            let on = self.findings_only;
            let label = if on {
                "Show all files"
            } else {
                "Show files with open findings"
            };
            filter = filter.child(titled(
                "pr-fl-findings-t",
                label,
                toggle_btn(
                    "pr-fl-findings",
                    IconName::HatGlasses,
                    on,
                    t.amber,
                    t.amber_d,
                    cx,
                )
                .debug_selector(move || format!("button:{label}:{on}"))
                .on_click(cx.listener(move |d, _, _, cx| {
                    d.findings_only = !on;
                    cx.notify();
                })),
            ));
        }
        if has_comments {
            let on = self.comments_only;
            let label = if on {
                "Show all files"
            } else {
                "Show files with unresolved comments"
            };
            filter = filter.child(titled(
                "pr-fl-comments-t",
                label,
                toggle_btn(
                    "pr-fl-comments",
                    IconName::MessageSquare,
                    on,
                    t.cyan,
                    t.cyan.fade(0.14),
                    cx,
                )
                .debug_selector(move || format!("button:{label}:{on}"))
                .on_click(cx.listener(move |d, _, _, cx| {
                    d.comments_only = !on;
                    cx.notify();
                })),
            ));
        }
        div()
            .debug_selector(|| "filelist".into())
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(px(width))
            .min_h_0()
            .min_w_0()
            .border_r_1()
            .border_color(hsla(t.line))
            .bg(hsla(t.bg))
            .child(handle)
            .child(filter)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(self.list.clone()),
            )
            .into_any_element()
    }
}

/// `.fl-filter .obs-icon-btn` with its `.on` tint.
fn toggle_btn(
    id: &'static str,
    name: IconName,
    on: bool,
    fg: recur_ui::Color,
    bg: recur_ui::Color,
    cx: &App,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    let t = Theme::of(cx).tokens;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(hsla(if on { fg } else { t.faint }))
        .when(on, |d| d.bg(hsla(bg)))
        .when(!on, |d| {
            d.hover(move |s| s.text_color(hsla(t.text)).bg(hsla(t.pane_2)))
        })
        .child(icon(name).size(px(15.)).stroke_width(2.2))
}
