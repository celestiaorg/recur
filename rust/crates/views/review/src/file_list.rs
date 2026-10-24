//! `ReviewFileList.svelte` inside its `.overlay-sb-shell > .fl-body` scroller: the Files
//! changed TOC, flat or as a tree with sticky open folders (`tree-sticky.ts`), finding and
//! unresolved-comment flags, uncommitted-edit dots, and reveal-on-spy.
//!
//! Rows have fixed heights (a file row is `3px + 12.5 × 1.35 + 3px`, a folder 24px), so the
//! list lays itself out and paints open folder headers in a layer at their sticky positions
//! (GPUI has no `position: sticky`).

use recur_ui::guard::Guarded;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    AnyElement, App, Context, Div, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollHandle, SharedString, Stateful, StatefulInteractiveElement, Styled, Window, canvas, div,
    point, px,
};
use recur_code::view::changed_tree::build_tree_from_paths;
use recur_code::view::finding_paths::findings_for_path;
use recur_domain::{FileDiff, PrComment, TreeEntry};
use recur_engine::view::comment_thread::{comment_count_for_path, comments_for_path};
use recur_ui::exact::vgap;
use recur_ui::file_icons::file_icon;
use recur_ui::fonts::Family;
use recur_ui::text::{LineHeight, TextStyled, text};
use recur_ui::tooltip::titled;
use recur_ui::widgets::scroll_area;
use recur_ui::{Color, IconName, Theme, Tokens, icon};
use recur_view_core::workspace_nav::scroll_delta_to_reveal;

use crate::findings::{FindingFlag, FindingsByPath, finding_flag_for_path, flag_of};
use crate::review_diff::{FileListMode, file_diff_path_label, file_diff_tree_label};
use crate::style::hsla;
use crate::tree_sticky::{StickyHeader, fade_hits_rows, sync_tree_sticky_fades};

/// `--tree-row-h`.
pub const TREE_ROW_H: f32 = 24.;
/// `--tree-fade`.
pub const TREE_FADE: f32 = 20.;
/// How far beyond the viewport the list keeps rows mounted.
const LIST_PAD_PX: f32 = 600.;
/// A file row: `padding: 3px 10px` around a 12.5px × 1.35 line.
pub const FILE_ROW_H: f32 = 3. + 12.5 * 1.35 + 3.;

/// `onSelect(path, { highlight })`.
/// Unresolved line notes grouped by file.
pub type CommentsByPath = Vec<(String, Vec<PrComment>)>;

pub type SelectFn = Rc<dyn Fn(String, Option<i64>, &mut Window, &mut App)>;

#[derive(Clone, Default, PartialEq)]
pub struct FileListProps {
    pub files: Vec<Rc<FileDiff>>,
    pub selected_path: String,
    pub mode: FileListMode,
    pub findings_by_path: Option<Rc<FindingsByPath>>,
    /// Unresolved line notes by path (PR detail).
    pub comments_by_path: Option<Rc<CommentsByPath>>,
    pub dirty_paths: Option<Rc<HashSet<String>>>,
    /// Bumped by the scroll spy: reveal `selected_path`.
    pub reveal: u64,
}

/// What [`ReviewFileList::model`] derives from the props and folds.
struct ListModel {
    rows: Vec<Row>,
    content_h: f32,
    by: HashMap<String, Rc<FileDiff>>,
}

/// One laid-out row.
#[derive(Clone)]
struct Row {
    entry: TreeEntry,
    depth: usize,
    top: f32,
    height: f32,
    /// An open folder: the bottom of its `.fl-branch`.
    branch_bottom: Option<f32>,
}

pub struct ReviewFileList {
    props: FileListProps,
    on_select: Option<SelectFn>,
    collapsed: HashSet<String>,
    /// Bumped when the props or the folds change (keys `model_memo`).
    model_gen: u64,
    model_memo: std::cell::RefCell<Option<(u64, Rc<ListModel>)>>,
    scroll: ScrollHandle,
    last_reveal: u64,
    pending_reveal: Option<String>,
    hovered: Option<String>,
}

impl ReviewFileList {
    pub fn new(props: FileListProps, on_select: Option<SelectFn>) -> Self {
        ReviewFileList {
            last_reveal: props.reveal,
            props,
            on_select,
            collapsed: HashSet::new(),
            model_gen: 0,
            model_memo: Default::default(),
            scroll: ScrollHandle::new(),
            pending_reveal: None,
            hovered: None,
        }
    }

    pub fn set_props(&mut self, props: FileListProps, cx: &mut Context<Self>) {
        if props == self.props {
            return;
        }
        if props.reveal != self.last_reveal {
            self.last_reveal = props.reveal;
            if props.reveal != 0 && !props.selected_path.is_empty() {
                if props.mode == FileListMode::Tree {
                    self.uncollapse_ancestors(&props.selected_path);
                }
                self.pending_reveal = Some(props.selected_path.clone());
            }
        }
        self.props = props;
        self.model_gen += 1;
        cx.notify();
    }

    pub fn set_on_select(&mut self, f: Option<SelectFn>) {
        self.on_select = f;
    }

    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll
    }

    pub fn collapsed(&self) -> &HashSet<String> {
        &self.collapsed
    }

    pub fn toggle_dir(&mut self, path: &str, cx: &mut Context<Self>) {
        if !self.collapsed.remove(path) {
            self.collapsed.insert(path.to_string());
        }
        self.model_gen += 1;
        cx.notify();
    }

    fn uncollapse_ancestors(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('/').collect();
        let mut acc = String::new();
        for part in &parts[..parts.len().saturating_sub(1)] {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(part);
            self.collapsed.remove(&acc);
        }
        self.model_gen += 1;
    }

    fn select(&self, path: &str, highlight: Option<i64>, window: &mut Window, cx: &mut App) {
        if let Some(f) = &self.on_select {
            f(path.to_string(), highlight, window, cx);
        }
    }

    /// The rows, their height and the files by path for the current props and folds
    /// (memoized: a large diff has thousands of paths to tree and index).
    fn model(&self) -> Rc<ListModel> {
        if let Some((g, m)) = self.model_memo.borrow().as_ref()
            && *g == self.model_gen
        {
            return m.clone();
        }
        let (rows, content_h) = self.layout();
        let by = self
            .props
            .files
            .iter()
            .map(|f| (f.path.clone(), f.clone()))
            .collect();
        let m = Rc::new(ListModel {
            rows,
            content_h,
            by,
        });
        *self.model_memo.borrow_mut() = Some((self.model_gen, m.clone()));
        m
    }

    fn finding_flags(&self) -> Vec<(String, FindingFlag)> {
        self.props
            .findings_by_path
            .as_deref()
            .map(|by| {
                by.iter()
                    .filter(|(_, fs)| !fs.is_empty())
                    .map(|(p, fs)| (p.clone(), flag_of(fs)))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn flag_for(&self, path: &str, is_dir: bool, flags: &[(String, FindingFlag)]) -> FindingFlag {
        if !is_dir && let Some(by) = self.props.findings_by_path.as_deref() {
            let fs = findings_for_path(by, path);
            if !fs.is_empty() {
                return flag_of(fs);
            }
        }
        finding_flag_for_path(path, is_dir, flags)
    }

    fn notes_for(&self, path: &str, is_dir: bool) -> usize {
        self.props
            .comments_by_path
            .as_deref()
            .map_or(0, |by| comment_count_for_path(by, path, is_dir))
    }

    fn jump_to_notes(&self, path: &str, window: &mut Window, cx: &mut App) {
        let first = self
            .props
            .comments_by_path
            .as_deref()
            .and_then(|by| comments_for_path(by, path).first().map(|c| c.line))
            .unwrap_or(0);
        self.select(path, Some(first), window, cx);
    }

    fn layout(&self) -> (Vec<Row>, f32) {
        let mut rows = Vec::new();
        let mut y = 0.;
        if self.props.mode == FileListMode::Flat {
            for f in &self.props.files {
                rows.push(Row {
                    entry: TreeEntry {
                        name: f.path.clone(),
                        path: f.path.clone(),
                        ..Default::default()
                    },
                    depth: 0,
                    top: y,
                    height: FILE_ROW_H,
                    branch_bottom: None,
                });
                y += FILE_ROW_H;
            }
            return (rows, y);
        }
        let paths: Vec<String> = self.props.files.iter().map(|f| f.path.clone()).collect();
        let tree = build_tree_from_paths(&paths);
        fn walk(
            nodes: &[TreeEntry],
            depth: usize,
            collapsed: &HashSet<String>,
            rows: &mut Vec<Row>,
            y: &mut f32,
        ) {
            for e in nodes {
                if e.dir {
                    let at = rows.len();
                    rows.push(Row {
                        entry: e.clone(),
                        depth,
                        top: *y,
                        height: TREE_ROW_H,
                        branch_bottom: None,
                    });
                    *y += TREE_ROW_H;
                    if !collapsed.contains(&e.path) {
                        walk(&e.children, depth + 1, collapsed, rows, y);
                        rows[at].branch_bottom = Some(*y);
                    }
                } else {
                    rows.push(Row {
                        entry: e.clone(),
                        depth,
                        top: *y,
                        height: FILE_ROW_H,
                        branch_bottom: None,
                    });
                    *y += FILE_ROW_H;
                }
            }
        }
        walk(&tree, 0, &self.collapsed, &mut rows, &mut y);
        (rows, y)
    }

    fn stats(&self, row: &Row, rows: &[Row], by: &HashMap<String, Rc<FileDiff>>) -> (i64, i64) {
        if !row.entry.dir {
            return by
                .get(&row.entry.path)
                .map_or((0, 0), |f| (f.diff.add, f.diff.del));
        }
        let prefix = format!("{}/", row.entry.path);
        fn sum(e: &TreeEntry, by: &HashMap<String, Rc<FileDiff>>) -> (i64, i64) {
            if !e.dir {
                return by
                    .get(e.path.as_str())
                    .map_or((0, 0), |f| (f.diff.add, f.diff.del));
            }
            e.children.iter().fold((0, 0), |(a, d), c| {
                let (x, y) = sum(c, by);
                (a + x, d + y)
            })
        }
        let _ = (rows, prefix);
        sum(&row.entry, by)
    }

    /// The row element without its position.
    #[allow(clippy::too_many_arguments)]
    fn row_el(
        &self,
        row: &Row,
        rows: &[Row],
        stuck: Option<(bool, bool)>,
        flags: &[(String, FindingFlag)],
        by: &HashMap<String, Rc<FileDiff>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = Theme::of(cx).tokens;
        let e = &row.entry;
        let tree = self.props.mode == FileListMode::Tree;
        let is_dir = e.dir;
        let open = is_dir && !self.collapsed.contains(&e.path);
        let on = !is_dir && e.path == self.props.selected_path;
        let f = (!is_dir).then(|| by.get(&e.path).cloned()).flatten();
        let f = f.as_deref();
        let st = f.map_or(String::new(), |f| f.status.clone());
        let (add, del) = self.stats(row, rows, by);
        let flag = self.flag_for(&e.path, is_dir, flags);
        let notes = self.notes_for(&e.path, is_dir);
        let dirty = !is_dir
            && self
                .props
                .dirty_paths
                .as_deref()
                .is_some_and(|d| d.contains(&e.path));
        let label_s = match (tree, f) {
            (false, Some(f)) => file_diff_path_label(&f.path, &f.old_path),
            (false, None) => e.path.clone(),
            (true, Some(f)) => file_diff_tree_label(&f.path, &f.old_path),
            (true, None) => e.name.clone(),
        };
        let title = if is_dir {
            e.path.clone()
        } else {
            let base = f.map_or(e.path.clone(), |f| {
                file_diff_path_label(&f.path, &f.old_path)
            });
            if dirty {
                format!("{base} · has uncommitted edits")
            } else {
                base
            }
        };
        let hovered = self.hovered.as_deref() == Some(e.path.as_str());
        let bg = if on || hovered {
            Some(t.pane_2)
        } else if is_dir && tree {
            Some(t.bg)
        } else {
            None
        };
        let path = e.path.clone();
        let id = SharedString::from(format!("fl-{}{}", if is_dir { "d:" } else { "f:" }, e.path));
        let sel = format!("fl-row:{}:{}", if is_dir { "dir" } else { "file" }, e.path);
        let mut d: Stateful<Div> = div()
            .id(id.clone())
            .debug_selector(move || sel.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(4.))
            .w_full()
            .px(px(10.))
            .cursor_pointer()
            .text_color(hsla(t.text))
            .when_some(bg, |d, c| d.bg(hsla(c)))
            .on_hover(cx.guarded_listener({
                let path = path.clone();
                move |l, on: &bool, _, cx| {
                    let next = on.then(|| path.clone());
                    if *on || l.hovered.as_deref() == Some(path.as_str()) {
                        l.hovered = next;
                        cx.notify();
                    }
                }
            }))
            .on_click(cx.guarded_listener({
                let path = path.clone();
                move |l, _, window, cx| {
                    if is_dir {
                        l.toggle_dir(&path, cx);
                    } else {
                        l.select(&path, None, window, cx);
                    }
                }
            }));
        d = if is_dir && tree {
            d.h(px(TREE_ROW_H))
        } else {
            d.py(px(3.))
        };
        let fp = text(label_s)
            .size(12.5)
            .line_height(LineHeight::Scale(1.35))
            .truncate();
        if tree {
            let mut main = div()
                .flex()
                .items_center()
                .gap(px(4.))
                .flex_1()
                .min_w_0()
                .pl(px(row.depth as f32 * 12.));
            main = if is_dir {
                main.child(
                    icon(IconName::ChevronDown)
                        .size(px(12.))
                        .stroke_width(2.4)
                        .color(hsla(t.faint))
                        .rotate(if open { 0. } else { -90. }),
                )
            } else {
                main.child(
                    div()
                        .flex_none()
                        .size(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(file_icon(e.name.clone())),
                )
            };
            d = d.child(main.child(div().flex_1().min_w_0().child(fp)));
        } else {
            d = d.child(div().flex_1().min_w_0().child(fp));
        }
        if notes > 0 {
            let p2 = path.clone();
            let c = div()
                .id(SharedString::from(format!("flc-{}", e.path)))
                .debug_selector({
                    let p = e.path.clone();
                    move || format!("obs-comment-flag:{p}")
                })
                .flex_none()
                .size(px(12.))
                .ml(px(2.))
                .when(!is_dir, |d| d.cursor_pointer())
                .child(
                    icon(IconName::MessageSquare)
                        .size(px(12.))
                        .stroke_width(2.2)
                        .color(hsla(if is_dir {
                            t.cyan.with_alpha(0.55)
                        } else {
                            t.cyan
                        })),
                )
                .on_click(cx.guarded_listener(move |l, _, window, cx| {
                    cx.stop_propagation();
                    if !is_dir {
                        l.jump_to_notes(&p2, window, cx);
                    }
                }));
            d = d.child(titled(
                SharedString::from(format!("flc-t-{}", e.path)),
                format!(
                    "{notes} unresolved comment{}",
                    if notes == 1 { "" } else { "s" }
                ),
                c,
            ));
        }
        if flag.count > 0 {
            let c = finding_flag_color(&flag.severity, &t);
            let c = if is_dir { c.with_alpha(c.a * 0.55) } else { c };
            d = d.child(titled(
                SharedString::from(format!("flf-t-{}", e.path)),
                format!(
                    "{} open finding{}",
                    flag.count,
                    if flag.count == 1 { "" } else { "s" }
                ),
                div()
                    .id(SharedString::from(format!("flf-{}", e.path)))
                    .debug_selector({
                        let p = e.path.clone();
                        move || format!("obs-finding-flag:{p}")
                    })
                    .flex_none()
                    .size(px(12.))
                    .ml(px(2.))
                    .child(
                        icon(IconName::HatGlasses)
                            .size(px(12.))
                            .stroke_width(2.2)
                            .color(hsla(c)),
                    ),
            ));
        }
        d = d.child(crate::style::add_del(
            add,
            del,
            Family::Ui,
            10.,
            5.,
            LineHeight::Scale(1.35),
            cx,
        ));
        d = d.child(fst(&st, dirty, &t));
        let _ = (window, stuck);
        titled(SharedString::from(format!("{id}-t")), title, d).into_any_element()
    }
}

fn finding_flag_color(sev: &str, t: &Tokens) -> Color {
    match sev {
        "high" | "critical" => t.coral,
        "low" | "info" => t.muted,
        _ => t.amber,
    }
}

/// `.fst`: the status letter (12px, centered), with the amber dot under it for uncommitted edits.
fn fst(st: &str, dirty: bool, t: &Tokens) -> AnyElement {
    let color = match st {
        "m" => Some(t.git_m),
        "a" => Some(t.git_a),
        "d" => Some(t.git_d),
        _ => None,
    };
    let letter = if st.is_empty() {
        "\u{a0}".to_string()
    } else {
        st.to_uppercase()
    };
    let mut label = text(letter)
        .size(10.)
        .weight(700.)
        .tracking(0.02)
        .line_height(LineHeight::Scale(1.35))
        .nowrap();
    if let Some(c) = color {
        label = label.color(c);
    }
    div()
        .relative()
        .flex_none()
        .w(px(12.))
        .flex()
        .justify_center()
        .child(label)
        .when(dirty, |d| {
            d.child(
                div()
                    .absolute()
                    .left(px(4.5))
                    .bottom(px(-2.))
                    .size(px(3.))
                    .rounded(px(1.5))
                    .bg(hsla(t.amber)),
            )
        })
        .into_any_element()
}

impl ReviewFileList {
    fn render_view(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let model = self.model();
        let (rows, content_h, by) = (&model.rows, model.content_h, &model.by);
        let flags = self.finding_flags();
        let view = self.scroll.bounds();
        let view_h = f32::from(view.size.height);
        let max_scroll = (content_h - view_h).max(0.);
        let scroll = (-f32::from(self.scroll.offset().y)).clamp(0., max_scroll);
        if let Some(path) = self.pending_reveal.take()
            && view_h > 0.
            && let Some(r) = rows.iter().find(|r| r.entry.path == path)
        {
            let ancestors = if self.props.mode == FileListMode::Tree {
                path.split('/')
                    .filter(|s| !s.is_empty())
                    .count()
                    .saturating_sub(1)
            } else {
                0
            };
            let delta = scroll_delta_to_reveal(
                r.top - scroll,
                r.top - scroll + r.height,
                0.,
                view_h,
                ancestors as f32 * TREE_ROW_H,
                0.,
            );
            if delta != 0. {
                let next = (scroll + delta).clamp(0., max_scroll);
                self.scroll.set_offset(point(px(0.), px(-next)));
                cx.notify();
            }
        }
        let tree = self.props.mode == FileListMode::Tree;
        // Sticky folder headers: where each open folder paints and which one fades.
        let open_dirs: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| tree && r.entry.dir && r.branch_bottom.is_some())
            .map(|(i, _)| i)
            .collect();
        let tops: Vec<f32> = open_dirs
            .iter()
            .map(|&i| {
                let r = &rows[i];
                let inset = r.depth as f32 * TREE_ROW_H;
                (scroll + inset)
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
                (!r.entry.dir && r.entry.path == self.props.selected_path)
                    || self.hovered.as_deref() == Some(r.entry.path.as_str())
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

        // Only rows near the viewport mount; exact spacers keep every row's position (a few
        // thousand rows laid out each frame cost seconds).
        let (lo, hi) = (scroll - LIST_PAD_PX, scroll + view_h.max(0.) + LIST_PAD_PX);
        let near = |r: &Row| r.top + r.height >= lo && r.top <= hi;
        let first = rows.iter().position(near);
        let last = rows.iter().rposition(near);
        let mut flow = div().relative().flex().flex_col().w_full();
        if let (Some(a), Some(b)) = (first, last) {
            flow = flow.child(vgap(rows[a].top, window));
            for (i, r) in rows.iter().enumerate().take(b + 1).skip(a) {
                if open_dirs.binary_search(&i).is_ok() {
                    flow = flow.child(div().flex_none().h(px(TREE_ROW_H)));
                } else {
                    flow = flow.child(self.row_el(r, rows, None, &flags, by, window, cx));
                }
            }
            flow = flow.child(vgap(content_h - (rows[b].top + rows[b].height), window));
        } else {
            flow = flow.child(vgap(content_h, window));
        }
        // The header layer: non-stuck headers in tree order, then stuck ones deepest first; a
        // header out of reach stays unmounted like its rows.
        let mut order: Vec<usize> = (0..open_dirs.len())
            .filter(|&k| !sticky[k].0 && tops[k] + TREE_ROW_H >= lo && tops[k] <= hi)
            .collect();
        let mut stuck: Vec<usize> = (0..open_dirs.len()).filter(|&k| sticky[k].0).collect();
        stuck.sort_by(|a, b| rows[open_dirs[*b]].depth.cmp(&rows[open_dirs[*a]].depth));
        order.extend(stuck);
        let mut layer: Vec<AnyElement> = Vec::new();
        for k in order {
            let r = &rows[open_dirs[k]];
            let (is_stuck, fade_under) = sticky[k];
            let row_el = self.row_el(
                r,
                rows,
                Some((is_stuck, fade_under)),
                &flags,
                by,
                window,
                cx,
            );
            let hovered = self.hovered.as_deref() == Some(r.entry.path.as_str());
            let fade_from = if hovered { t.pane_2 } else { t.bg };
            let fade = fade_under.then(|| {
                div()
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
                    .child(vgap(tops[k], window))
                    .child(div().relative().child(row_el).children(fade))
                    .into_any_element(),
            );
        }
        flow = flow.children(layer);
        let me = cx.entity().downgrade();
        let probe = canvas(
            move |_, _, cx| {
                let me = me.clone();
                cx.defer(move |cx| {
                    if let Some(me) = me.upgrade() {
                        me.update(cx, |l, cx| {
                            if l.pending_reveal.is_some() {
                                cx.notify();
                            }
                        });
                    }
                });
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0();
        div()
            .debug_selector(|| "fl-body".into())
            .relative()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                scroll_area("fl-body")
                    .overlay()
                    .track(&self.scroll)
                    .child(flow),
            )
            .child(probe)
    }
}

impl Render for ReviewFileList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The files view caches the list.
        recur_ui::style::cached_root(recur_ui::guard::view(self, window, cx, Self::render_view))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, Entity, TestAppContext};

    struct Host(Entity<ReviewFileList>);

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(320.))
                .h(px(400.))
                .flex()
                .flex_col()
                .child(self.0.clone())
        }
    }

    #[gpui::test]
    fn a_long_list_mounts_only_the_rows_near_the_viewport(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            recur_ui::init(cx);
        });
        let files: Vec<Rc<FileDiff>> = (0..2000)
            .map(|i| {
                Rc::new(FileDiff {
                    path: format!("f{i:04}.rs"),
                    status: "m".into(),
                    ..FileDiff::default()
                })
            })
            .collect();
        let props = FileListProps {
            files,
            mode: FileListMode::Flat,
            ..FileListProps::default()
        };
        let (host, cx) =
            cx.add_window_view(move |_, cx| Host(cx.new(|_| ReviewFileList::new(props, None))));
        cx.run_until_parked();
        let row = |cx: &mut gpui::VisualTestContext, i: usize| {
            let sel: &'static str = Box::leak(format!("fl-row:file:f{i:04}.rs").into_boxed_str());
            cx.debug_bounds(sel)
        };
        assert!(row(cx, 0).is_some());
        assert!(row(cx, 100).is_none(), "row 100 is far below the fold");
        let list = host.read_with(cx, |h, _| h.0.clone());
        let scroll = list.read_with(cx, |l, _| l.scroll.clone());
        // The spacers keep the full height: the last row is reachable and where it belongs.
        let max = f32::from(scroll.max_offset().y);
        assert!((max - (2000. * FILE_ROW_H - 400.)).abs() < 1., "{max}");
        scroll.set_offset(point(px(0.), px(-max)));
        list.update(cx, |_, cx| cx.notify());
        cx.run_until_parked();
        let last = row(cx, 1999).expect("the last row mounts at the bottom");
        assert!(row(cx, 0).is_none());
        let body = cx.debug_bounds("fl-body").expect("list body");
        assert!((f32::from(last.bottom()) - f32::from(body.bottom())).abs() < 1.);
    }
}
