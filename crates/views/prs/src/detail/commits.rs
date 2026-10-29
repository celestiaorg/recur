//! The Commits pane: every commit (newest first) with its verified and check marks, and one
//! commit's header and windowed diff.

use recur_ui::guard::Guarded;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_domain::{Commit, Pr, PrSource};
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::tooltip::titled;
use recur_ui::{IconName, Theme, icon};
use recur_view_review::commit_meta::{commit_byline, commit_sha};
use recur_view_review::commits::{RowOpts, cav, commit_row, empty_state};
use recur_view_review::stack::CardProps;

use super::PrDetail;
use super::conversation::commit_check;
use crate::style::hsla;

/// The commits pane's padding, above the list.
const PANE_PAD: f32 = 18.;
/// A `commit_row`: 11px padding around a 12.5px message and a 10px byline at 1.45 line height,
/// and its top border. The last row adds its bottom border.
// simplify: rows are assumed one height; a byline that wraps (a very narrow pane) makes the
// spacers drift. Measure laid-out rows (as the review stack does) if that shows up.
const ROW_H: f32 = 57.625;

/// The rows within a viewport of the visible ones, `scroll` px past the list's top in a
/// viewport `h` tall: `(first, end)`. Long PRs lay out the rows on screen, not every commit.
fn row_window(scroll: f32, h: f32, n: usize) -> (usize, usize) {
    let from = ((scroll - h) / ROW_H).floor().max(0.) as usize;
    let to = ((scroll + 2. * h) / ROW_H).ceil().max(0.) as usize;
    (from.min(n), to.min(n).max(from.min(n)))
}

impl PrDetail {
    /// `.tl-verified` and `commitCheck` after a commit's message.
    fn commit_marks(
        &self,
        c: &Commit,
        key: &str,
        window: &mut Window,
        cx: &App,
    ) -> Vec<AnyElement> {
        let t = Theme::of(cx).tokens;
        let mut out = Vec::new();
        if c.verified {
            out.push(
                div()
                    .flex_none()
                    .py(px(1.))
                    .px(px(7.))
                    .rounded(px(99.))
                    .border_1()
                    .border_color(hsla(recur_ui::rgba(63, 185, 80, 0.45)))
                    .child(
                        text("Verified")
                            .mono()
                            .size(9.)
                            .color(recur_ui::hex(0x3fb950))
                            .nowrap(),
                    )
                    .into_any_element(),
            );
        }
        out.push(commit_check(&c.check_status, key, window, cx));
        let _ = t;
        out
    }

    /// The `.commit-hd` of PR detail's commit view: message, byline with the file count,
    /// verified and check marks, the short hash and its copy button, then the body.
    fn commit_head(
        &mut self,
        c: &Commit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let files = self.detail_files(cx).len();
        let suffix = if !self.commit_loading || files > 0 {
            format!("· {files} file{}", if files == 1 { "" } else { "s" })
        } else {
            String::new()
        };
        let sha = commit_sha(c);
        let copied = self.copied.contains(&sha);
        let copy_label = if copied { "Copied" } else { "Copy full SHA" };
        let s2 = sha.clone();
        let copy = titled(
            "commit-hd-copy-t",
            copy_label,
            div()
                .id("commit-hd-copy")
                .debug_selector(move || format!("chash-copy:{copy_label}"))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(5.))
                .cursor_pointer()
                .text_color(hsla(if copied { t.emerald } else { t.faint }))
                .hover(move |s| {
                    if copied {
                        s.bg(hsla(t.pane_2))
                    } else {
                        s.text_color(hsla(t.text)).bg(hsla(t.pane_2))
                    }
                })
                .child(if copied {
                    icon(IconName::Check).size(px(12.)).stroke_width(2.4)
                } else {
                    icon(IconName::Copy).size(px(12.))
                })
                .on_click(cx.guarded_listener(move |d, _, _, cx| {
                    cx.stop_propagation();
                    d.copied.copy(&s2, |d: &mut PrDetail| &mut d.copied, cx);
                    cx.notify();
                })),
        );
        let mut top = div()
            .flex()
            .items_start()
            .gap(px(12.))
            .child(cav(c, 30., 8., 10., cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        text(c.message.clone())
                            .id("commit-hd-t")
                            .size(14.)
                            .weight(600.)
                            .break_anywhere(),
                    )
                    .child(
                        div().mt(px(4.)).child(
                            Text::from_spans(byline(c, &suffix, &t))
                                .id("commit-hd-by")
                                .family(Family::Mono)
                                .size(10.5)
                                .color(t.faint),
                        ),
                    ),
            );
        top = top.children(self.commit_marks(c, "commit-hd", window, cx));
        top = top
            .child(
                recur_ui::style::card(t.pane_2, t.line, 6.)
                    .flex_none()
                    .ml_auto()
                    .py(px(3.))
                    .px(px(9.))
                    .child(
                        text(c.hash.clone())
                            .mono()
                            .size(11.)
                            .color(t.muted)
                            .nowrap(),
                    ),
            )
            .child(copy);
        let mut col = recur_ui::style::card(t.pane, t.line, 9.)
            .debug_selector(|| "commit-hd".into())
            .flex()
            .flex_col()
            .gap(px(12.))
            .py(px(14.))
            .px(px(15.))
            .mb(px(14.))
            .child(top);
        if !c.body.is_empty() {
            col = col.child(
                div().pt(px(2.)).child(
                    Text::from_spans(vec![span(c.body.clone())])
                        .id("commit-hd-body")
                        .size(12.5)
                        .line_height(LineHeight::Scale(1.5))
                        .color(t.muted)
                        .pre_wrap()
                        .break_anywhere(),
                ),
            );
        }
        col.into_any_element()
    }

    /// The Commits pane.
    pub(crate) fn commits_pane(
        &mut self,
        pr: Option<&Pr>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let commits = self.commits(cx);
        let detail = self.commit_idx.is_some();
        let mut wrap = div()
            .w_full()
            .max_w(px(if detail { 1560. } else { 840. }))
            .mx_auto()
            .p(px(PANE_PAD))
            .flex()
            .flex_col();
        let Some(pr) = pr else {
            let scroll = self.commits_scroll.clone();
            return self.pane("commits", &scroll, wrap, cx);
        };
        match self.commit_idx {
            None if self.commits_pending(cx) => {
                wrap = wrap.child(super::skeleton::commits_skel(window, cx));
            }
            None => {
                let n = commits.len();
                let scroll = -f32::from(self.commits_scroll.offset().y);
                let vp = f32::from(self.commits_scroll.bounds().size.height);
                let vp = if vp > 0. {
                    vp
                } else {
                    f32::from(window.viewport_size().height)
                };
                let (first, end) = row_window(scroll - PANE_PAD, vp, n);
                let mut list = div()
                    .debug_selector(|| "commits-list".into())
                    .flex()
                    .flex_col();
                if first > 0 {
                    list = list.child(div().flex_none().h(px(first as f32 * ROW_H)));
                }
                for (i, c) in commits.iter().enumerate().take(end).skip(first) {
                    let sha = commit_sha(c);
                    let opts = RowOpts {
                        id: SharedString::from(format!("commit-{i}")),
                        first: i == 0,
                        last: i + 1 == n,
                        copied: self.copied.contains(&sha),
                        chevron: true,
                        trailing: self.commit_marks(c, &format!("row-{i}"), window, cx),
                    };
                    let me = cx.entity().downgrade();
                    let me2 = cx.entity().downgrade();
                    list = list.child(commit_row(
                        c,
                        opts,
                        Some(Rc::new(move |_, cx| {
                            let _ = me.update(cx, |d, cx| d.open_commit(i, cx));
                        })),
                        Rc::new(move |_, cx| {
                            let sha = sha.clone();
                            let _ = me2.update(cx, |d, cx| {
                                d.copied.copy(&sha, |d: &mut PrDetail| &mut d.copied, cx);
                                cx.notify();
                            });
                        }),
                        cx,
                    ));
                }
                if end < n {
                    list = list.child(div().flex_none().h(px((n - end) as f32 * ROW_H + 1.)));
                }
                let hint = if pr.source == PrSource::GitHub {
                    "Commits on this pull request. Click any to see its diff."
                } else {
                    "Every commit is the loop's work — agent-authored, one per iteration. Click any to see its diff. The messages trace how it converged: build → fix on review → fix on review → tests."
                };
                wrap = wrap.child(
                    div().child(list).child(
                        div().pt(px(14.)).px(px(2.)).child(
                            text(hint)
                                .mono()
                                .size(10.5)
                                .color(t.faint)
                                .id("commits-hint"),
                        ),
                    ),
                );
            }
            Some(i) => {
                if let Some(c) = commits.get(i).cloned() {
                    let nav = div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .mb(px(14.))
                        .child(
                            div()
                                .id("cback")
                                .debug_selector(|| "cback".into())
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .cursor_pointer()
                                .text_color(hsla(t.muted))
                                .hover(move |s| s.text_color(hsla(t.text)))
                                .child(
                                    recur_ui::style::button_label("← all commits", 11., 400.)
                                        .family(Family::Mono),
                                )
                                .on_click(cx.guarded_listener(|d, _, _, cx| {
                                    d.commit_idx = None;
                                    d.discard_commit_diff();
                                    cx.notify();
                                })),
                        )
                        .child(self.diff_layout_seg(false, cx));
                    let head = self.commit_head(&c, window, cx);
                    if self.commit_loading {
                        wrap = wrap
                            .child(nav)
                            .child(head)
                            .child(empty_state("Loading diff…", cx));
                    } else if self.commit_error {
                        wrap = wrap
                            .child(nav)
                            .child(head)
                            .child(empty_state("Could not load this commit's diff", cx));
                    } else {
                        let files: Vec<Rc<recur_domain::FileDiff>> =
                            self.detail_files(cx).into_iter().map(Rc::new).collect();
                        self.commit_stack.set_files(files.clone());
                        let state = self.state().clone();
                        let ws = pr.workspace_id.clone();
                        let mut hooks = recur_view_review::diff_file::DiffFileHooks::default();
                        if !ws.is_empty() {
                            hooks.open_file = Some(Rc::new(move |p, l, _, cx| {
                                PrDetail::open_in_workspace(&state, &p, l, cx)
                            }));
                        }
                        self.commit_stack.set_props(CardProps {
                            layout: self.diff_layout,
                            note_mode: None,
                            comments: true,
                            collapsible: true,
                            open_file_title: Some("Open in workspace".into()),
                            hooks,
                            ..Default::default()
                        });
                        let empty = files
                            .is_empty()
                            .then(|| empty_state("No files in this commit", cx));
                        let stack = self.commit_stack.render(
                            vec![nav.into_any_element(), head],
                            empty,
                            |d: &mut PrDetail| &mut d.commit_stack,
                            Rc::new(|d: &mut PrDetail, path, next, cx| {
                                d.commit_stack.toggle_collapsed(&path, next);
                                cx.notify();
                            }),
                            window,
                            cx,
                        );
                        let top = self.commit_stack.pane.scroll_top();
                        let fade = recur_view_review::scroll_fade::is_scrolled_from_top(
                            top,
                            recur_view_review::scroll_fade::SCROLL_FADE_SLACK,
                        );
                        return div()
                            .debug_selector(move || {
                                format!("prpane:commits{}", if fade { ":fade-top" } else { "" })
                            })
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child(stack)
                            .when(fade, |d| d.child(super::render::top_fade(cx)))
                            .into_any_element();
                    }
                }
            }
        }
        let scroll = self.commits_scroll.clone();
        self.pane("commits", &scroll, wrap, cx)
    }
}

/// The `.cm-by` spans: role and amber iteration, or the author byline, then `suffix`. Svelte
/// drops the whitespace an `{#if}` block opens with, so the separators read `Coder· iter`.
fn byline(c: &Commit, suffix: &str, t: &recur_ui::Tokens) -> Vec<Span> {
    if !c.author_role.is_empty() && c.author_role != "Author" {
        let mut v = vec![span(c.author_role.clone())];
        if !c.iter_label.is_empty() {
            v.push(span("· "));
            v.push(span(c.iter_label.clone()).color(t.amber));
        }
        if !suffix.is_empty() {
            v.push(span(suffix.to_string()));
        }
        return v;
    }
    vec![span(format!("{}{suffix}", commit_byline(c)))]
}
