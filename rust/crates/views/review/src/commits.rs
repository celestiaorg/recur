//! `CommitsView.svelte` and `CommitRow.svelte`: the commit list (uncommitted work first), and
//! the commit header Files changed shows for a commit scope. `.commit*` rules from `_prs.scss`.

use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Div, EventEmitter, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, Render, SharedString, StatefulInteractiveElement,
    Styled, StyledImage, Task, Window, div, img, px,
};
use recur_domain::Commit;
use recur_engine::view::js;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, Span, Text, TextStyled, span, text};
use recur_ui::tooltip::titled;
use recur_ui::{Color, IconName, Theme, icon};

use crate::commit_meta::{commit_byline, commit_sha};
use crate::style::{CAV_INK, hsla};

/// `.empty` with its `.et` line.
pub fn empty_state(s: impl Into<SharedString>, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let s: SharedString = s.into();
    div()
        .debug_selector({
            let s = s.clone();
            move || format!("empty:{s}")
        })
        .flex()
        .flex_col()
        .items_center()
        .gap(px(7.))
        .pt(px(72.))
        .pb(px(72.))
        .px(px(20.))
        .child(
            text(s)
                .size(14.)
                .weight(600.)
                .color(t.text)
                .align(recur_ui::text::Align::Center)
                .id("empty-et"),
        )
        .into_any_element()
}

/// "No files match — clear filter".
pub fn empty_clear_filter(
    on_clear: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    div()
        .debug_selector(|| "empty:No files match".into())
        .flex()
        .flex_col()
        .items_center()
        .gap(px(7.))
        .pt(px(72.))
        .pb(px(72.))
        .px(px(20.))
        .child(
            div()
                .flex()
                .items_baseline()
                .child(text("No files match — ").size(14.).weight(600.).nowrap())
                .child(
                    div()
                        .id("clear-filter")
                        .debug_selector(|| "clear-filter".into())
                        .cursor_pointer()
                        .text_color(hsla(t.amber))
                        .hover(|s| s.text_color(hsla(t.text)))
                        .child(text("clear filter").size(14.).weight(600.).nowrap())
                        .on_click(on_clear),
                ),
        )
        .into_any_element()
}

/// The `.cav` fill: the commit's `color` (`var(--cyan)`, a hex) in the current theme.
pub fn cav_color(c: &Commit, cx: &App) -> Option<Color> {
    Theme::of(cx).resolve_css_color(&c.color)
}

/// `.commit .cav` (26px, radius 7) or `.commit-hd .cav` (30px, radius 8).
pub fn cav(c: &Commit, size: f32, radius: f32, font: f32, cx: &App) -> Div {
    let fill = cav_color(c, cx);
    let mut d = div()
        .relative()
        .flex_none()
        .size(px(size))
        .rounded(px(radius))
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .when_some(fill, |d, f| d.bg(hsla(f)));
    if c.avatar_url.is_empty() {
        d = d.child(
            text(c.agent_code.clone())
                .family(Family::Mono)
                .size(font)
                .weight(700.)
                .line_height(LineHeight::Scale(1.45))
                .color(CAV_INK)
                .nowrap(),
        );
    } else {
        d = d.child(
            img(SharedString::from(c.avatar_url.clone()))
                .size_full()
                .object_fit(ObjectFit::Cover),
        );
    }
    d
}

/// `.cav.wip`: amber with "WIP" in `--on-amber`, 8px.
pub fn cav_wip(size: f32, radius: f32, cx: &App) -> Div {
    let t = Theme::of(cx).tokens;
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(radius))
        .bg(hsla(t.amber))
        .flex()
        .items_center()
        .justify_center()
        .child(
            text("WIP")
                .family(Family::Mono)
                .size(8.)
                .weight(700.)
                .tracking(0.02)
                .line_height(LineHeight::Scale(1.45))
                .color(t.on_amber)
                .nowrap(),
        )
}

/// The `.cm-by` byline: the role with its amber iteration, or the author.
fn byline_spans(c: &Commit, suffix: &str, cx: &App) -> Vec<Span> {
    let t = Theme::of(cx).tokens;
    if !c.author_role.is_empty() && c.author_role != "Author" {
        let mut v = vec![span(c.author_role.clone())];
        if !c.iter_label.is_empty() {
            v.push(span(" · "));
            v.push(span(c.iter_label.clone()).color(t.amber));
        }
        if !suffix.is_empty() {
            v.push(span(suffix.to_string()));
        }
        return v;
    }
    vec![span(format!("{}{suffix}", commit_byline(c)))]
}

/// `.commit-hd` for a commit scope (message, byline with the file count, hash, body).
pub fn commit_header(c: &Commit, suffix: &str, cx: &App) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let top = div()
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
                        .line_height(LineHeight::Scale(1.45))
                        .break_anywhere(),
                )
                .child(
                    div().mt(px(4.)).child(
                        Text::from_spans(byline_spans(c, suffix, cx))
                            .id("commit-hd-by")
                            .family(Family::Mono)
                            .size(10.5)
                            .line_height(LineHeight::Scale(1.45))
                            .color(t.faint),
                    ),
                ),
        )
        .child(
            recur_ui::style::card(t.pane_2, t.line, 6.)
                .debug_selector(|| "commit-hd-chash".into())
                .flex_none()
                .ml_auto()
                .py(px(3.))
                .px(px(9.))
                .child(
                    text(c.hash.clone())
                        .family(Family::Mono)
                        .size(11.)
                        .line_height(LineHeight::Scale(1.45))
                        .color(t.muted)
                        .nowrap(),
                ),
        );
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

/// Copied-SHA flashes (`copied` per row, 1.2s).
#[derive(Default)]
pub struct CopiedShas {
    copied: HashSet<String>,
    timers: Vec<Task<()>>,
}

impl CopiedShas {
    pub fn contains(&self, sha: &str) -> bool {
        self.copied.contains(sha)
    }

    /// `copySha`: copy and flash; `on_done` clears the flash on the host.
    pub fn copy<H: 'static>(
        &mut self,
        sha: &str,
        project: fn(&mut H) -> &mut CopiedShas,
        cx: &mut Context<H>,
    ) {
        if sha.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(sha.to_string()));
        self.copied.insert(sha.to_string());
        let sha = sha.to_string();
        self.timers.push(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            let _ = this.update(cx, |h, cx| {
                project(h).copied.remove(&sha);
                cx.notify();
            });
        }));
        cx.notify();
    }
}

/// Options for [`commit_row`].
pub struct RowOpts {
    pub id: SharedString,
    pub first: bool,
    pub last: bool,
    pub copied: bool,
    pub chevron: bool,
    /// The `trailing` snippet (PR detail: Verified, check status), before the diff counts.
    pub trailing: Vec<AnyElement>,
}

/// `CommitRow.svelte`. `on_open` makes the row a button; `on_copy` copies the full SHA.
pub fn commit_row(
    c: &Commit,
    opts: RowOpts,
    on_open: Option<Rc<dyn Fn(&mut Window, &mut App)>>,
    on_copy: Rc<dyn Fn(&mut Window, &mut App)>,
    cx: &App,
) -> AnyElement {
    let t = Theme::of(cx).tokens;
    let sha = commit_sha(c);
    let interactive = on_open.is_some();
    let copy_label = if opts.copied {
        "Copied"
    } else {
        "Copy full SHA"
    };
    let copy = div()
        .id(SharedString::from(format!("{}-copy", opts.id)))
        .debug_selector(|| "chash-copy".into())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(22.))
        .rounded(px(5.))
        .cursor_pointer()
        .text_color(hsla(if opts.copied { t.emerald } else { t.faint }))
        .hover(move |s| {
            if opts.copied {
                s.bg(hsla(t.pane_2))
            } else {
                s.text_color(hsla(t.text)).bg(hsla(t.pane_2))
            }
        })
        .child(if opts.copied {
            icon(IconName::Check).size(px(12.)).stroke_width(2.4)
        } else {
            icon(IconName::Copy).size(px(12.))
        })
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_copy(window, cx)
        });
    let body = div()
        .flex_1()
        .min_w_0()
        .child(
            text(c.message.clone())
                .size(12.5)
                .line_height(LineHeight::Scale(1.45))
                .truncate(),
        )
        .child(
            div().mt(px(2.)).child(
                Text::from_spans(byline_spans(c, "", cx))
                    .id(SharedString::from(format!("{}-by", opts.id)))
                    .family(Family::Mono)
                    .size(10.)
                    .line_height(LineHeight::Scale(1.45))
                    .color(t.faint),
            ),
        );
    let group = SharedString::from(format!("{}-g", opts.id));
    let row = row_frame(
        opts.id.clone(),
        group.clone(),
        opts.first,
        opts.last,
        t.pane,
        t.pane_2,
        &t,
    )
    .debug_selector({
        let m = c.message.clone();
        move || format!("commit:{m}")
    })
    .child(cav(c, 26., 7., 9., cx))
    .child(body)
    .children(opts.trailing)
    .child(
        div()
            .flex()
            .flex_none()
            .justify_end()
            .w(px(64.))
            .child(crate::style::add_del(
                c.diff.add,
                c.diff.del,
                Family::Mono,
                10.,
                5.,
                LineHeight::Scale(1.45),
                cx,
            )),
    )
    .child(chash(&c.hash, &t))
    .child(titled(
        SharedString::from(format!("{}-copy-t", opts.id)),
        copy_label,
        copy,
    ))
    .child(
        div().flex_none().w(px(38.)).child(
            text(c.when.clone())
                .family(Family::Mono)
                .size(10.)
                .line_height(LineHeight::Scale(1.45))
                .color(t.faint)
                .align(recur_ui::text::Align::Right)
                .nowrap(),
        ),
    )
    .when(opts.chevron, |d| d.child(chevron(group.clone(), &t)));
    let _ = sha;
    match on_open {
        Some(open) if interactive => row
            .cursor_pointer()
            .on_click(move |_, window, cx| open(window, cx))
            .into_any_element(),
        _ => row.into_any_element(),
    }
}

fn chash(hash: &str, t: &recur_ui::Tokens) -> AnyElement {
    recur_ui::style::card(t.pane_2, t.line, 5.)
        .flex_none()
        .py(px(2.))
        .px(px(7.))
        .child(
            text(hash.to_string())
                .family(Family::Mono)
                .size(10.5)
                .line_height(LineHeight::Scale(1.45))
                .color(t.muted)
                .nowrap(),
        )
        .into_any_element()
}

fn chevron(group: SharedString, t: &recur_ui::Tokens) -> AnyElement {
    let (faint, muted) = (t.faint, t.muted);
    div()
        .flex_none()
        .text_color(hsla(faint))
        .group_hover(group, move |s| s.text_color(hsla(muted)))
        .child(icon(IconName::ChevronRight).size(px(14.)))
        .into_any_element()
}

/// `.commit`: the bordered row with the timeline line through its avatar column.
fn row_frame(
    id: SharedString,
    group: SharedString,
    first: bool,
    last: bool,
    bg: Color,
    hover: Color,
    t: &recur_ui::Tokens,
) -> gpui_kit::Stateful<Div> {
    let line = div()
        .absolute()
        .left(px(26.))
        .w(px(1.))
        .bg(hsla(t.line))
        .when(first, |d| d.top(gpui_kit::relative(0.5)))
        .when(!first, |d| d.top_0())
        .when(last, |d| d.bottom(gpui_kit::relative(0.5)))
        .when(!last, |d| d.bottom_0());
    let radius = |d: gpui_kit::Stateful<Div>| match (first, last) {
        (true, true) => d.rounded(px(9.)),
        (true, false) => d.rounded_t(px(9.)),
        (false, true) => d.rounded_b(px(9.)),
        (false, false) => d,
    };
    radius(
        div()
            .id(id)
            .group(group)
            .relative()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(11.))
            .px(px(14.))
            .border_l_1()
            .border_r_1()
            .border_t_1()
            .when(last, |d| d.border_b_1())
            .border_color(hsla(t.line))
            .bg(hsla(bg))
            .hover(move |s| s.bg(hsla(hover))),
    )
    .child(line)
}

/// Events from [`CommitsView`].
#[derive(Clone, Debug, PartialEq)]
pub enum CommitsEvent {
    /// `onOpenCommit`.
    Open(Commit),
    /// `onOpenWorking`.
    OpenWorking,
}

/// `CommitsView.svelte`.
pub struct CommitsView {
    pub commits: Vec<Commit>,
    pub loading: bool,
    pub error: bool,
    pub base_label: String,
    pub working_dirty_count: usize,
    copied: CopiedShas,
    scroll: gpui_kit::ScrollHandle,
}

impl EventEmitter<CommitsEvent> for CommitsView {}

impl Default for CommitsView {
    fn default() -> Self {
        Self::new()
    }
}

impl CommitsView {
    pub fn new() -> Self {
        CommitsView {
            commits: Vec::new(),
            loading: false,
            error: false,
            base_label: String::new(),
            working_dirty_count: 0,
            copied: CopiedShas::default(),
            scroll: gpui_kit::ScrollHandle::new(),
        }
    }

    pub fn set(
        &mut self,
        commits: Vec<Commit>,
        loading: bool,
        error: bool,
        base_label: String,
        working_dirty_count: usize,
        cx: &mut Context<Self>,
    ) {
        self.commits = commits;
        self.loading = loading;
        self.error = error;
        self.base_label = base_label;
        self.working_dirty_count = working_dirty_count;
        cx.notify();
    }

    fn copy(&mut self, sha: &str, cx: &mut Context<Self>) {
        self.copied
            .copy(sha, |v: &mut CommitsView| &mut v.copied, cx);
    }
}

impl Render for CommitsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Theme::of(cx).tokens;
        let vs = js::trim(&self.base_label).to_string();
        let show_working = self.working_dirty_count > 0;
        let list_empty = self.commits.is_empty() && !show_working;
        let mut wrap = div()
            .id("commits-wrap")
            .debug_selector(|| "commits-wrap".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(18.));
        if self.loading && list_empty {
            wrap = wrap.child(empty_state("Loading commits…", cx));
        } else if self.error && list_empty {
            wrap = wrap.child(empty_state("Could not load commits", cx));
        } else if list_empty {
            wrap = wrap.child(empty_state(
                if vs.is_empty() {
                    "No commits yet".to_string()
                } else {
                    format!("No commits since {vs}")
                },
                cx,
            ));
        } else {
            let mut list = div()
                .debug_selector(|| "commits-list".into())
                .flex()
                .flex_col();
            let n = self.commits.len() + usize::from(show_working);
            if show_working {
                let group = SharedString::from("commit-wip-g");
                let wip_bg = t.amber.mix(0.08, t.pane);
                let wip_hover = t.amber.mix(0.14, t.pane_2);
                list = list.child(
                    row_frame(
                        "commit-wip".into(),
                        group.clone(),
                        true,
                        n == 1,
                        wip_bg,
                        wip_hover,
                        &t,
                    )
                    .debug_selector(|| "commit:working".into())
                    .cursor_pointer()
                    .child(cav_wip(26., 7., cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                text("Uncommitted changes")
                                    .size(12.5)
                                    .line_height(LineHeight::Scale(1.45))
                                    .truncate(),
                            )
                            .child(
                                div().mt(px(2.)).child(
                                    text("Working tree vs HEAD")
                                        .family(Family::Mono)
                                        .size(10.)
                                        .line_height(LineHeight::Scale(1.45))
                                        .color(t.faint)
                                        .nowrap(),
                                ),
                            ),
                    )
                    .child(chash("—", &t))
                    .child(chevron(group, &t))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(CommitsEvent::OpenWorking))),
                );
            }
            for (i, c) in self.commits.clone().iter().enumerate() {
                let k = i + usize::from(show_working);
                let sha = commit_sha(c);
                let me = cx.entity().downgrade();
                let me2 = me.clone();
                let c2 = c.clone();
                let sha2 = sha.clone();
                list = list.child(commit_row(
                    c,
                    RowOpts {
                        id: SharedString::from(format!("commit-{}", c.hash)),
                        first: k == 0,
                        last: k + 1 == n,
                        copied: self.copied.contains(&sha),
                        chevron: true,
                        trailing: Vec::new(),
                    },
                    Some(Rc::new(move |_, cx| {
                        let c = c2.clone();
                        let _ = me.update(cx, |_, cx| cx.emit(CommitsEvent::Open(c)));
                    })),
                    Rc::new(move |_, cx| {
                        let sha = sha2.clone();
                        let _ = me2.update(cx, |v, cx| v.copy(&sha, cx));
                    }),
                    cx,
                ));
            }
            wrap = wrap.child(list);
            if self.error && self.commits.is_empty() {
                wrap = wrap.child(empty_state("Could not load commits", cx));
            } else if self.loading && self.commits.is_empty() {
                wrap = wrap.child(empty_state("Loading commits…", cx));
            } else {
                let hint = if vs.is_empty() {
                    "Recent commits on this branch".to_string()
                } else {
                    format!("Commits on this branch since {vs}")
                };
                wrap = wrap.child(
                    div().pt(px(14.)).px(px(2.)).child(
                        text(format!("{hint}. Click any to see its diff."))
                            .id("commits-hint")
                            .family(Family::Mono)
                            .size(10.5)
                            .color(t.faint),
                    ),
                );
            }
        }
        div()
            .debug_selector(|| "review:commits".into())
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(hsla(t.bg))
            .child(wrap)
    }
}
