//! The project tree's model: which groups, projects and workspace rows the sidebar shows, and
//! their vertical layout. Every row has a fixed CSS height, so positions are computed here
//! instead of measured, and shared by rendering, the sticky headers, drag targeting and
//! scroll-into-view.

use std::collections::{BTreeMap, HashMap};

use recur_domain::{Project, ProjectGroup, Workspace};
use recur_view_core::AppState;
use recur_view_core::modes::parked_needs_you_by_workspace;
use recur_view_core::workspace_nav::{
    is_nested_child, main_scan_id, nest_workspaces, split_scheduled, unique_refs,
};

/// `--rail-group-h`.
pub const GROUP_H: f32 = 26.;
/// `--rail-proj-h`.
pub const PROJ_H: f32 = 30.;
/// `.tloop` height.
pub const ROW_H: f32 = 28.;
/// `.tsched-h` height and margin-top.
pub const SCHED_H: f32 = 22.;
pub const SCHED_MT: f32 = 2.;
/// `.tgroup { padding-bottom: 4px }`.
pub const GROUP_PB: f32 = 4.;
/// `.rail-tree { padding-bottom: 8px }`.
pub const TREE_PB: f32 = 8.;
/// `--rail-fade`.
pub const FADE: f32 = 20.;

/// `.tproj-empty`: padding 4/6 around one mono 10.5px line at the inherited 1.45 (WebKit
/// floors a numeric line height to 1/64 px).
pub fn empty_h() -> f32 {
    4. + 6. + (10.5_f32 * 1.45 * 64.).floor() / 64.
}

/// A task row as the tree paints it.
#[derive(Clone, Debug)]
pub struct Row {
    pub ws: Workspace,
    pub nested: bool,
    pub nested_last: bool,
    /// A root row followed by its first child (the stem down to the children).
    pub has_child_below: bool,
}

/// `loopRows`: the nesting flags each row paints with.
fn rows(list: &[&Workspace], nest: bool) -> Vec<Row> {
    list.iter()
        .enumerate()
        .map(|(i, l)| {
            let nested = nest || is_nested_child(*l, list);
            let next = list.get(i + 1);
            let nested_last = if nest {
                i + 1 == list.len()
            } else {
                nested
                    && !next
                        .is_some_and(|n| n.parent_id == l.parent_id && is_nested_child(*n, list))
            };
            let has_child_below =
                !nested && next.is_some_and(|n| nest || is_nested_child(*n, list));
            Row {
                ws: (*l).clone(),
                nested,
                nested_last,
                has_child_below,
            }
        })
        .collect()
}

#[derive(Clone, Debug)]
pub struct ProjectNode {
    pub project: Project,
    pub main: Option<Workspace>,
    pub scheduled: Vec<Row>,
    pub regular: Vec<Row>,
    pub collapsed: bool,
    pub sched_closed: bool,
    pub in_group: bool,
    /// Unseen chats and parked prompts for the header chip.
    pub unseen: i64,
    pub parked: usize,
    pub sched_unseen: i64,
    pub sched_parked: usize,
}

impl ProjectNode {
    /// The `.tloops` height under the header.
    pub fn loops_h(&self) -> f32 {
        if self.collapsed {
            return 0.;
        }
        let mut h = ROW_H;
        if !self.scheduled.is_empty() {
            h += SCHED_MT + SCHED_H;
            if !self.sched_closed {
                h += ROW_H * self.scheduled.len() as f32;
            }
        }
        h += ROW_H * self.regular.len() as f32;
        if self.regular.is_empty() && self.scheduled.is_empty() && self.project.is_git {
            h += empty_h();
        }
        h
    }

    pub fn height(&self) -> f32 {
        PROJ_H + self.loops_h()
    }
}

#[derive(Clone, Debug)]
pub struct GroupNode {
    pub group: ProjectGroup,
    pub members: Vec<ProjectNode>,
    pub collapsed: bool,
    pub unseen: i64,
    pub parked: usize,
}

/// What the tree shows, in paint order.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub groups: Vec<GroupNode>,
    pub ungrouped: Vec<ProjectNode>,
}

/// Collapse state the sidebar keeps per project, group and scheduled section.
#[derive(Clone, Debug, Default)]
pub struct Collapse {
    pub projects: HashMap<String, bool>,
    pub groups: HashMap<String, bool>,
    /// Scheduled sections default to collapsed.
    pub scheduled: HashMap<String, bool>,
}

impl Tree {
    pub fn build(s: &AppState, collapse: &Collapse) -> Tree {
        let show_hidden = s.show_hidden_projects();
        let shown = |p: &Project| !p.hidden || show_hidden;
        let groups = unique_refs(&s.project_groups);
        let projects = unique_refs(&s.projects);
        let parked =
            parked_needs_you_by_workspace(&s.workspaces, &s.user_questions, &s.permission_requests);
        let parked_of = |id: &str| parked.get(id).copied().unwrap_or(0);
        let visible: Vec<&Workspace> = unique_refs(&s.workspaces)
            .into_iter()
            .filter(|w| !w.archived && !w.is_main && w.host_pr_id.trim().is_empty())
            .collect();
        let mut main_by_project: BTreeMap<&str, &Workspace> = BTreeMap::new();
        for w in &s.workspaces {
            if w.is_main {
                main_by_project.insert(&w.project_id, w);
            }
        }
        let node = |p: &Project, in_group: bool| -> ProjectNode {
            let mine: Vec<&Workspace> = visible
                .iter()
                .copied()
                .filter(|w| w.project_id == p.id)
                .collect();
            let (regular, scheduled) = split_scheduled(&mine);
            let regular = nest_workspaces(&regular);
            let scheduled = nest_workspaces(&scheduled);
            let main = main_by_project.get(p.id.as_str()).map(|w| (*w).clone());
            let main_parked = main.as_ref().map(|m| parked_of(&m.id)).unwrap_or(0);
            let sched_parked: usize = scheduled.iter().map(|l| parked_of(&l.id)).sum();
            let sched_unseen: i64 = scheduled.iter().map(|l| s.attention.workspace(&l.id)).sum();
            ProjectNode {
                project: p.clone(),
                parked: main_parked
                    + regular.iter().map(|l| parked_of(&l.id)).sum::<usize>()
                    + sched_parked,
                unseen: s.attention.project(&p.id),
                main,
                regular: rows(&regular, false),
                scheduled: rows(&scheduled, true),
                collapsed: collapse.projects.get(&p.id).copied().unwrap_or(false),
                sched_closed: collapse.scheduled.get(&p.id).copied().unwrap_or(true),
                in_group,
                sched_unseen,
                sched_parked,
            }
        };
        let group_ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        let tree_groups = groups
            .iter()
            .map(|g| {
                let members: Vec<ProjectNode> = projects
                    .iter()
                    .filter(|p| shown(p) && p.group_id == g.id)
                    .map(|p| node(p, true))
                    .collect();
                let member_ids: Vec<&str> = members.iter().map(|m| m.project.id.as_str()).collect();
                let unseen = member_ids.iter().map(|id| s.attention.project(id)).sum();
                let parked = s
                    .workspaces
                    .iter()
                    .filter(|w| {
                        !w.archived
                            && w.host_pr_id.trim().is_empty()
                            && member_ids.contains(&w.project_id.as_str())
                    })
                    .map(|w| parked_of(&w.id))
                    .sum();
                GroupNode {
                    group: (*g).clone(),
                    collapsed: collapse.groups.get(&g.id).copied().unwrap_or(false),
                    members,
                    unseen,
                    parked,
                }
            })
            .collect();
        let ungrouped = projects
            .iter()
            .filter(|p| {
                shown(p) && (p.group_id.is_empty() || !group_ids.contains(&p.group_id.as_str()))
            })
            .map(|p| node(p, false))
            .collect();
        Tree {
            groups: tree_groups,
            ungrouped,
        }
    }
}

/// A block's vertical extent in tree content coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Span {
    pub top: f32,
    pub height: f32,
}

impl Span {
    pub fn bottom(&self) -> f32 {
        self.top + self.height
    }
}

/// The block a drag lifted out of the tree: its slot becomes a placeholder of the block's
/// height at pickup.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hole<'a> {
    pub group: bool,
    pub id: &'a str,
    pub height: f32,
}

/// Positions of every group, project and workspace row.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub groups: Vec<(String, Span)>,
    /// Project id → (block, in group).
    pub projects: Vec<(String, Span, bool)>,
    /// Workspace id (and the main row's scan id) → row top.
    pub rows: HashMap<String, f32>,
    /// The drag placeholder.
    pub hole: Option<Span>,
    pub content_h: f32,
}

impl Layout {
    pub fn of(tree: &Tree, hole: Option<Hole>) -> Layout {
        let mut l = Layout::default();
        let mut y = 0.;
        let is_hole = |group: bool, id: &str| hole.filter(|h| h.group == group && h.id == id);
        let place = |p: &ProjectNode, top: f32, l: &mut Layout| -> f32 {
            if let Some(h) = is_hole(false, &p.project.id) {
                l.hole = Some(Span {
                    top,
                    height: h.height,
                });
                return h.height;
            }
            l.projects.push((
                p.project.id.clone(),
                Span {
                    top,
                    height: p.height(),
                },
                p.in_group,
            ));
            if p.collapsed {
                return p.height();
            }
            let mut ry = top + PROJ_H;
            l.rows.insert(main_scan_id(&p.project.id), ry);
            if let Some(m) = &p.main {
                l.rows.insert(m.id.clone(), ry);
            }
            ry += ROW_H;
            if !p.scheduled.is_empty() {
                ry += SCHED_MT + SCHED_H;
                for r in &p.scheduled {
                    if !p.sched_closed {
                        l.rows.insert(r.ws.id.clone(), ry);
                        ry += ROW_H;
                    }
                }
            }
            for r in &p.regular {
                l.rows.insert(r.ws.id.clone(), ry);
                ry += ROW_H;
            }
            p.height()
        };
        for g in &tree.groups {
            if let Some(h) = is_hole(true, &g.group.id) {
                l.hole = Some(Span {
                    top: y,
                    height: h.height,
                });
                y += h.height;
                continue;
            }
            let start = y;
            if g.collapsed {
                y += GROUP_H;
            } else if g.members.is_empty() {
                y += GROUP_H + empty_h();
            } else {
                y += GROUP_H;
                for p in &g.members {
                    y += place(p, y, &mut l);
                }
            }
            y += GROUP_PB;
            l.groups.push((
                g.group.id.clone(),
                Span {
                    top: start,
                    height: y - start,
                },
            ));
        }
        for p in &tree.ungrouped {
            y += place(p, y, &mut l);
        }
        l.content_h = y + TREE_PB;
        l
    }

    pub fn group(&self, id: &str) -> Option<Span> {
        self.groups.iter().find(|(g, _)| g == id).map(|(_, s)| *s)
    }

    pub fn project(&self, id: &str) -> Option<(Span, bool)> {
        self.projects
            .iter()
            .find(|(p, _, _)| p == id)
            .map(|(_, s, g)| (*s, *g))
    }
}

/// Where a sticky header sits for `scroll`: pinned at `inset` below the viewport top while
/// its block is on screen, pushed up by the block's end, never above its natural place.
pub fn sticky_top(natural: f32, block_bottom: f32, header_h: f32, scroll: f32, inset: f32) -> f32 {
    (scroll + inset).min(block_bottom - header_h).max(natural)
}

/// `updateTreeEdges`' "stuck" test: the header sits at its sticky inset (within 1.5px) while
/// the tree is scrolled.
pub fn is_stuck(top: f32, scroll: f32, inset: f32, fade_top: bool) -> bool {
    fade_top && (top - scroll - inset).abs() <= 1.5
}

/// The overlay scrollbar thumb: `(height, top)` for a viewport of `track` px.
pub fn thumb(scroll: f32, content: f32, track: f32) -> (f32, f32) {
    let track = track.max(1.);
    let max_scroll = content - track;
    let ratio = (track / content.max(1.)).min(1.);
    let h = (track * ratio).max(24.);
    let top = if max_scroll > 0. {
        scroll / max_scroll * (track - h)
    } else {
        0.
    };
    (h, top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticky_math() {
        // Natural place while not scrolled past.
        assert_eq!(sticky_top(100., 300., 30., 50., 0.), 100.);
        // Pinned at the top.
        assert_eq!(sticky_top(100., 300., 30., 150., 0.), 150.);
        // Pushed up by the block's end.
        assert_eq!(sticky_top(100., 300., 30., 290., 0.), 270.);
        // In a group, below the group header.
        assert_eq!(sticky_top(100., 300., 30., 150., 26.), 176.);
        assert!(is_stuck(150., 150., 0., true));
        assert!(!is_stuck(150., 150., 0., false));
        assert!(!is_stuck(100., 50., 0., true));
    }

    #[test]
    fn thumb_geometry() {
        assert_eq!(thumb(0., 100., 200.), (200., 0.));
        let (h, top) = thumb(100., 400., 200.);
        assert_eq!(h, 100.);
        assert_eq!(top, 50.);
        assert_eq!(thumb(0., 10_000., 200.).0, 24.);
    }

    #[test]
    fn empty_line_height_is_floored() {
        assert_eq!(empty_h(), 10. + 974. / 64.);
    }
}
