//! Sidebar order and keyboard workspace cycling.

use std::collections::{HashMap, HashSet};

use recur_domain::{Project, ProjectGroup, Workspace};

/// Sentinel cycle id for a project's Working copy row before `OpenMainWorkspace`.
pub const MAIN_SCAN_PREFIX: &str = "main:";

pub fn main_scan_id(project_id: &str) -> String {
    format!("{MAIN_SCAN_PREFIX}{project_id}")
}

pub fn project_id_from_main_scan(id: &str) -> Option<&str> {
    id.strip_prefix(MAIN_SCAN_PREFIX)
}

/// The fields sidebar navigation reads from a workspace.
pub trait NavWorkspace {
    fn id(&self) -> &str;
    fn project_id(&self) -> &str;
    fn archived(&self) -> bool;
    fn is_main(&self) -> bool;
    fn host_pr_id(&self) -> &str;
    /// Fan-out child: nested under this workspace in the sidebar.
    fn parent_id(&self) -> &str;
    /// Occurrence workspace created by a scheduled prompt.
    fn schedule_id(&self) -> &str;
}

impl NavWorkspace for Workspace {
    fn id(&self) -> &str {
        &self.id
    }
    fn project_id(&self) -> &str {
        &self.project_id
    }
    fn archived(&self) -> bool {
        self.archived
    }
    fn is_main(&self) -> bool {
        self.is_main
    }
    fn host_pr_id(&self) -> &str {
        &self.host_pr_id
    }
    fn parent_id(&self) -> &str {
        &self.parent_id
    }
    fn schedule_id(&self) -> &str {
        &self.schedule_id
    }
}

/// Anything keyed by an id (`uniqueById`).
pub trait HasId {
    fn key(&self) -> &str;
}

impl HasId for Workspace {
    fn key(&self) -> &str {
        &self.id
    }
}
impl HasId for Project {
    fn key(&self) -> &str {
        &self.id
    }
}
impl HasId for ProjectGroup {
    fn key(&self) -> &str {
        &self.id
    }
}
impl HasId for recur_domain::Pr {
    fn key(&self) -> &str {
        &self.id
    }
}

/// Sidebar order for one project's task rows: each visible parent followed by its children
/// (store order within each tier). A child whose parent is not in the list (archived, other
/// project) renders as a root.
pub fn nest_workspaces<'a, T: NavWorkspace>(items: &[&'a T]) -> Vec<&'a T> {
    let ids: HashSet<&str> = items.iter().map(|w| w.id()).collect();
    let mut kids: HashMap<&str, Vec<&'a T>> = HashMap::new();
    let mut roots = Vec::new();
    for &w in items {
        let pid = w.parent_id().trim();
        if !pid.is_empty() && pid != w.id() && ids.contains(pid) {
            kids.entry(pid).or_default().push(w);
        } else {
            roots.push(w);
        }
    }
    let mut out = Vec::with_capacity(items.len());
    for r in roots {
        out.push(r);
        if let Some(list) = kids.get(r.id()) {
            out.extend(list.iter().copied());
        }
    }
    out
}

/// True when this row should render nested under a parent in the same list.
pub fn is_nested_child<T: NavWorkspace>(w: &T, items: &[&T]) -> bool {
    let pid = w.parent_id().trim();
    !pid.is_empty() && pid != w.id() && items.iter().any(|o| o.id() == pid)
}

/// A project's task rows split into human-created vs scheduled (and their fan-out children).
pub fn split_scheduled<'a, T: NavWorkspace>(items: &[&'a T]) -> (Vec<&'a T>, Vec<&'a T>) {
    let mut scheduled: HashSet<&str> = items
        .iter()
        .filter(|w| !w.schedule_id().trim().is_empty())
        .map(|w| w.id())
        .collect();
    let mut grew = true;
    while grew {
        grew = false;
        for w in items {
            if scheduled.contains(w.id()) {
                continue;
            }
            let pid = w.parent_id().trim();
            if !pid.is_empty() && scheduled.contains(pid) {
                scheduled.insert(w.id());
                grew = true;
            }
        }
    }
    items
        .iter()
        .copied()
        .partition(|w| !scheduled.contains(w.id()))
}

/// First-wins unique by id. Sidebar rows are keyed, and a list with the same id twice would
/// paint duplicate rows. Pass `warn` only at store-write sites.
pub fn unique_by_id<T: HasId + Clone>(items: &[T], warn: bool) -> Vec<T> {
    let mut seen = HashSet::new();
    let mut dropped = Vec::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if !seen.insert(item.key().to_string()) {
            if warn {
                dropped.push(item.key().to_string());
            }
            continue;
        }
        out.push(item.clone());
    }
    if !dropped.is_empty() {
        tracing::warn!("uniqueById: dropped duplicate ids {dropped:?}");
    }
    out
}

/// [`unique_by_id`] over references, for read paths that must not clone.
pub fn unique_refs<T: HasId>(items: &[T]) -> Vec<&T> {
    let mut seen = HashSet::new();
    items.iter().filter(|i| seen.insert(i.key())).collect()
}

/// Project ids in the order the sidebar tree paints them.
pub fn sidebar_project_ids(projects: &[&Project], groups: &[ProjectGroup]) -> Vec<String> {
    let group_ids: HashSet<&str> = groups.iter().map(|g| g.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut by_group: HashMap<&str, Vec<&Project>> = HashMap::new();
    for &p in projects {
        if !seen.insert(p.id.as_str()) {
            continue;
        }
        let gid = if !p.group_id.is_empty() && group_ids.contains(p.group_id.as_str()) {
            p.group_id.as_str()
        } else {
            ""
        };
        by_group.entry(gid).or_default().push(p);
    }
    let mut ordered = Vec::new();
    for g in unique_refs(groups) {
        if let Some(members) = by_group.get(g.id.as_str()) {
            ordered.extend(members.iter().map(|p| p.id.clone()));
        }
    }
    if let Some(ungrouped) = by_group.get("") {
        ordered.extend(ungrouped.iter().map(|p| p.id.clone()));
    }
    ordered
}

/// Navigable workspace ids in sidebar order: grouped projects, then ungrouped; within a
/// project the working copy (isMain), then scheduled workspaces, then other task workspaces in
/// store order (newest first).
pub fn sidebar_workspace_ids<T: NavWorkspace>(
    workspaces: &[T],
    projects: &[&Project],
    groups: &[ProjectGroup],
) -> Vec<String> {
    let project_order = sidebar_project_ids(projects, groups);
    let known: HashSet<&str> = project_order.iter().map(String::as_str).collect();
    let mut mains: HashMap<&str, &str> = HashMap::new();
    let mut has_main = HashSet::new();
    let mut loops_by_project: HashMap<&str, Vec<&T>> = HashMap::new();
    let mut seen = HashSet::new();
    for w in workspaces {
        if !seen.insert(w.id()) || !known.contains(w.project_id()) {
            continue;
        }
        if w.is_main() {
            has_main.insert(w.project_id());
            if !w.archived() {
                mains.insert(w.project_id(), w.id());
            }
            continue;
        }
        if w.archived() || !w.host_pr_id().trim().is_empty() {
            continue;
        }
        loops_by_project.entry(w.project_id()).or_default().push(w);
    }
    let mut ids = Vec::new();
    for pid in &project_order {
        if let Some(main) = mains.get(pid.as_str()) {
            ids.push(main.to_string());
        } else if !has_main.contains(pid.as_str()) {
            ids.push(main_scan_id(pid));
        }
        if let Some(loops) = loops_by_project.get(pid.as_str()) {
            let (regular, scheduled) = split_scheduled(loops);
            ids.extend(
                nest_workspaces(&scheduled)
                    .into_iter()
                    .map(|w| w.id().to_string()),
            );
            ids.extend(
                nest_workspaces(&regular)
                    .into_iter()
                    .map(|w| w.id().to_string()),
            );
        }
    }
    ids
}

/// Next/prev id with wrap. `delta` is +1 (down / next) or -1 (up / previous).
pub fn neighbor_workspace_id(ids: &[String], current_id: &str, delta: i32) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let i = if current_id.is_empty() {
        None
    } else {
        ids.iter().position(|id| id == current_id)
    };
    let Some(i) = i else {
        return Some(if delta >= 0 {
            ids[0].clone()
        } else {
            ids[ids.len() - 1].clone()
        });
    };
    if ids.len() == 1 {
        return None;
    }
    let n = ids.len() as i64;
    let step = if delta >= 0 { 1 } else { -1 };
    Some(ids[((i as i64 + step + n) % n) as usize].clone())
}

/// How long the user must stay on one workspace before a peeked (auto-expanded)
/// project/group counts as a stop. Shorter than this is pass-through: leaving re-collapses.
pub const SCAN_COMMIT_MS: u64 = 1000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanPeek {
    pub peeked_projects: Vec<String>,
    pub peeked_groups: Vec<String>,
    pub current_project_id: String,
    pub current_group_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanStep {
    pub project_id: String,
    pub group_id: String,
    pub project_collapsed: bool,
    pub group_collapsed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanDiff {
    pub expand_projects: Vec<String>,
    pub expand_groups: Vec<String>,
    pub collapse_projects: Vec<String>,
    pub collapse_groups: Vec<String>,
}

/// What [`scan_commit`] decided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanCommit {
    pub next: ScanPeek,
    pub persist_projects: Vec<String>,
    pub persist_groups: Vec<String>,
    pub collapse_projects: Vec<String>,
    pub collapse_groups: Vec<String>,
}

/// An insertion-ordered set, as JS `Set` iterates.
fn remove(list: &mut Vec<String>, id: &str) -> bool {
    match list.iter().position(|x| x == id) {
        Some(i) => {
            list.remove(i);
            true
        }
        None => false,
    }
}

fn add(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|x| x == id) {
        list.push(id.to_string());
    }
}

/// Expand collapsed ancestors for this land; re-collapse uncommitted peeks we left.
pub fn scan_land(prev: &ScanPeek, step: &ScanStep) -> (ScanPeek, ScanDiff) {
    let mut peeked_projects = prev.peeked_projects.clone();
    let mut peeked_groups = prev.peeked_groups.clone();
    let mut diff = ScanDiff::default();

    if !prev.current_project_id.is_empty()
        && prev.current_project_id != step.project_id
        && remove(&mut peeked_projects, &prev.current_project_id)
    {
        diff.collapse_projects.push(prev.current_project_id.clone());
    }
    if !prev.current_group_id.is_empty()
        && prev.current_group_id != step.group_id
        && remove(&mut peeked_groups, &prev.current_group_id)
    {
        diff.collapse_groups.push(prev.current_group_id.clone());
    }

    let group_peeked = peeked_groups.contains(&step.group_id);
    if !step.group_id.is_empty() && (step.group_collapsed || group_peeked) {
        if step.group_collapsed && !group_peeked {
            diff.expand_groups.push(step.group_id.clone());
        }
        add(&mut peeked_groups, &step.group_id);
    }
    let project_peeked = peeked_projects.contains(&step.project_id);
    if !step.project_id.is_empty() && (step.project_collapsed || project_peeked) {
        if step.project_collapsed && !project_peeked {
            diff.expand_projects.push(step.project_id.clone());
        }
        add(&mut peeked_projects, &step.project_id);
    }

    (
        ScanPeek {
            peeked_projects,
            peeked_groups,
            current_project_id: step.project_id.clone(),
            current_group_id: step.group_id.clone(),
        },
        diff,
    )
}

/// Dwell on the current row, or a click/palette landing. Leaving the peeked project/group
/// (including an empty landing) re-collapses instead of persisting. `landing` of `None` means
/// "stay where the scan is"; `Some(("", ""))` is an empty landing.
pub fn scan_commit(prev: &ScanPeek, landing: Option<(&str, &str)>) -> ScanCommit {
    let (dest_project, dest_group) =
        landing.unwrap_or((&prev.current_project_id, &prev.current_group_id));
    let stay_project = !dest_project.is_empty() && dest_project == prev.current_project_id;
    let stay_group = !dest_group.is_empty() && dest_group == prev.current_group_id;
    let next = ScanPeek {
        peeked_projects: Vec::new(),
        peeked_groups: Vec::new(),
        current_project_id: dest_project.to_string(),
        current_group_id: dest_group.to_string(),
    };
    ScanCommit {
        next,
        persist_projects: if stay_project {
            prev.peeked_projects.clone()
        } else {
            Vec::new()
        },
        persist_groups: if stay_group {
            prev.peeked_groups.clone()
        } else {
            Vec::new()
        },
        collapse_projects: if stay_project {
            Vec::new()
        } else {
            prev.peeked_projects.clone()
        },
        collapse_groups: if stay_group {
            Vec::new()
        } else {
            prev.peeked_groups.clone()
        },
    }
}

/// The user toggled collapse themselves: stop treating it as a pass-through peek.
pub fn scan_forget(prev: &ScanPeek, project_id: Option<&str>, group_id: Option<&str>) -> ScanPeek {
    let mut next = prev.clone();
    if let Some(id) = project_id.filter(|s| !s.is_empty()) {
        next.peeked_projects.retain(|x| x != id);
    }
    if let Some(id) = group_id.filter(|s| !s.is_empty()) {
        next.peeked_groups.retain(|x| x != id);
    }
    next
}

/// Pixels to add to a scroller's scroll offset so a row sits in the visible band (below
/// sticky headers, above a bottom fade). Coordinates are in one space.
pub fn scroll_delta_to_reveal(
    row_top: f32,
    row_bottom: f32,
    view_top: f32,
    view_bottom: f32,
    top_inset: f32,
    bottom_inset: f32,
) -> f32 {
    let visible_top = view_top + top_inset;
    let visible_bottom = view_bottom - bottom_inset;
    if visible_bottom <= visible_top || row_top < visible_top {
        return row_top - visible_top;
    }
    if row_bottom > visible_bottom {
        return row_bottom - visible_bottom;
    }
    0.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Default)]
    struct W {
        id: String,
        project_id: String,
        archived: bool,
        is_main: bool,
        host_pr_id: String,
        parent_id: String,
        schedule_id: String,
    }

    impl NavWorkspace for W {
        fn id(&self) -> &str {
            &self.id
        }
        fn project_id(&self) -> &str {
            &self.project_id
        }
        fn archived(&self) -> bool {
            self.archived
        }
        fn is_main(&self) -> bool {
            self.is_main
        }
        fn host_pr_id(&self) -> &str {
            &self.host_pr_id
        }
        fn parent_id(&self) -> &str {
            &self.parent_id
        }
        fn schedule_id(&self) -> &str {
            &self.schedule_id
        }
    }

    fn ws(id: &str, project: &str) -> W {
        W {
            id: id.into(),
            project_id: project.into(),
            ..W::default()
        }
    }

    fn with(mut w: W, f: impl FnOnce(&mut W)) -> W {
        f(&mut w);
        w
    }

    fn ids<T: NavWorkspace>(v: &[&T]) -> Vec<String> {
        v.iter().map(|w| w.id().to_string()).collect()
    }

    fn proj(id: &str, group: &str) -> Project {
        Project {
            id: id.into(),
            group_id: group.into(),
            ..Project::default()
        }
    }

    fn group(id: &str) -> ProjectGroup {
        ProjectGroup {
            id: id.into(),
            ..ProjectGroup::default()
        }
    }

    #[test]
    fn nest_children_follow_parent_orphans_are_roots() {
        let items = [
            with(ws("child-b", "p"), |w| w.parent_id = "parent".into()),
            ws("other", "p"),
            ws("parent", "p"),
            with(ws("child-a", "p"), |w| w.parent_id = "parent".into()),
            with(ws("orphan", "p"), |w| w.parent_id = "gone".into()),
        ];
        let refs: Vec<&W> = items.iter().collect();
        assert_eq!(
            ids(&nest_workspaces(&refs)),
            ["other", "parent", "child-b", "child-a", "orphan"]
        );
        assert!(is_nested_child(&items[0], &refs));
        assert!(!is_nested_child(&items[4], &refs));
    }

    #[test]
    fn split_scheduled_groups_occurrences_and_children() {
        let items = [
            with(ws("sched", "p"), |w| w.schedule_id = "daily".into()),
            ws("task", "p"),
            with(ws("child", "p"), |w| w.parent_id = "sched".into()),
            with(ws("orphan", "p"), |w| w.parent_id = "gone".into()),
        ];
        let refs: Vec<&W> = items.iter().collect();
        let (regular, scheduled) = split_scheduled(&refs);
        assert_eq!(ids(&regular), ["task", "orphan"]);
        assert_eq!(ids(&scheduled), ["sched", "child"]);
    }

    fn sidebar_projects() -> (Vec<Project>, Vec<ProjectGroup>) {
        (
            vec![
                proj("storefront", ""),
                proj("payments", "g-api"),
                proj("marketing", ""),
                proj("billing", "g-api"),
            ],
            vec![group("g-api")],
        )
    }

    #[test]
    fn sidebar_ids_groups_first_then_ungrouped_main_then_tasks() {
        let (projects, groups) = sidebar_projects();
        let p: Vec<&Project> = projects.iter().collect();
        let workspaces = [
            ws("task-sf-new", "storefront"),
            ws("task-pay", "payments"),
            with(ws("main-sf", "storefront"), |w| w.is_main = true),
            ws("task-sf-old", "storefront"),
            with(ws("main-pay", "payments"), |w| w.is_main = true),
            ws("task-mkt", "marketing"),
            ws("task-bill", "billing"),
            with(ws("archived-sf", "storefront"), |w| w.archived = true),
            ws("orphan", "gone"),
        ];
        assert_eq!(
            sidebar_workspace_ids(&workspaces, &p, &groups),
            [
                "main-pay",
                "task-pay",
                "main:billing",
                "task-bill",
                "main-sf",
                "task-sf-new",
                "task-sf-old",
                "main:marketing",
                "task-mkt"
            ]
        );
    }

    #[test]
    fn sidebar_ids_skip_archived_and_insert_sentinel() {
        let projects = [proj("storefront", "")];
        let p: Vec<&Project> = projects.iter().collect();
        assert_eq!(
            sidebar_workspace_ids(
                &[
                    with(ws("a", "storefront"), |w| w.archived = true),
                    ws("b", "storefront")
                ],
                &p,
                &[]
            ),
            ["main:storefront", "b"]
        );
    }

    #[test]
    fn sidebar_ids_skip_pr_chat_hosts() {
        let projects = [proj("storefront", "")];
        let p: Vec<&Project> = projects.iter().collect();
        assert_eq!(
            sidebar_workspace_ids(
                &[
                    with(ws("host", "storefront"), |w| w.host_pr_id = "pr-1".into()),
                    ws("task", "storefront")
                ],
                &p,
                &[]
            ),
            ["main:storefront", "task"]
        );
    }

    #[test]
    fn sidebar_ids_scheduled_under_working_copy() {
        let projects = [proj("storefront", "")];
        let p: Vec<&Project> = projects.iter().collect();
        assert_eq!(
            sidebar_workspace_ids(
                &[
                    with(ws("sched", "storefront"), |w| w.schedule_id =
                        "daily".into()),
                    ws("task", "storefront"),
                    with(ws("child", "storefront"), |w| w.parent_id = "sched".into()),
                ],
                &p,
                &[]
            ),
            ["main:storefront", "sched", "child", "task"]
        );
    }

    #[test]
    fn sidebar_ids_drop_duplicates() {
        let projects = [proj("storefront", ""), proj("storefront", "")];
        let p: Vec<&Project> = projects.iter().collect();
        assert_eq!(
            sidebar_workspace_ids(
                &[
                    ws("task-a", "storefront"),
                    ws("task-a", "storefront"),
                    ws("task-b", "storefront")
                ],
                &p,
                &[]
            ),
            ["main:storefront", "task-a", "task-b"]
        );
    }

    #[test]
    fn unique_keeps_first_and_treats_empty_as_a_key() {
        let a = |id: &str, n: &str| Project {
            id: id.into(),
            name: n.into(),
            ..Project::default()
        };
        let out = unique_by_id(&[a("a", "1"), a("a", "2"), a("b", "3")], false);
        assert_eq!(
            out.iter()
                .map(|p| (p.id.as_str(), p.name.as_str()))
                .collect::<Vec<_>>(),
            [("a", "1"), ("b", "3")]
        );
        let out = unique_by_id(&[a("", "1"), a("", "2"), a("x", "3")], true);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn neighbor_wraps() {
        let list: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let n = |cur: &str, d| neighbor_workspace_id(&list, cur, d);
        assert_eq!(n("a", 1).as_deref(), Some("b"));
        assert_eq!(n("c", 1).as_deref(), Some("a"));
        assert_eq!(n("a", -1).as_deref(), Some("c"));
        assert_eq!(n("b", -1).as_deref(), Some("a"));
        assert_eq!(n("", 1).as_deref(), Some("a"));
        assert_eq!(n("", -1).as_deref(), Some("c"));
        assert_eq!(n("missing", 1).as_deref(), Some("a"));
        assert_eq!(n("missing", -1).as_deref(), Some("c"));
    }

    #[test]
    fn neighbor_empty_and_singleton() {
        let only = vec!["only".to_string()];
        assert_eq!(neighbor_workspace_id(&[], "a", 1), None);
        assert_eq!(neighbor_workspace_id(&only, "only", 1), None);
        assert_eq!(neighbor_workspace_id(&only, "only", -1), None);
        assert_eq!(neighbor_workspace_id(&only, "", 1).as_deref(), Some("only"));
        assert_eq!(
            neighbor_workspace_id(&only, "other", -1).as_deref(),
            Some("only")
        );
    }

    fn step(p: &str, g: &str, pc: bool, gc: bool) -> ScanStep {
        ScanStep {
            project_id: p.into(),
            group_id: g.into(),
            project_collapsed: pc,
            group_collapsed: gc,
        }
    }

    #[test]
    fn scan_expands_and_recollapses_when_leaving_uncommitted() {
        let (first, diff) = scan_land(&ScanPeek::default(), &step("a", "", true, false));
        assert_eq!(diff.expand_projects, ["a"]);
        assert_eq!(first.peeked_projects, ["a"]);
        let (still, diff) = scan_land(&first, &step("a", "", false, false));
        assert!(diff.expand_projects.is_empty() && diff.collapse_projects.is_empty());
        assert_eq!(still.peeked_projects, ["a"]);
        let (left, diff) = scan_land(&still, &step("b", "", true, false));
        assert_eq!(diff.collapse_projects, ["a"]);
        assert_eq!(diff.expand_projects, ["b"]);
        assert_eq!(left.peeked_projects, ["b"]);
    }

    #[test]
    fn scan_commit_keeps_current_peek_open() {
        let (first, _) = scan_land(&ScanPeek::default(), &step("a", "", true, false));
        let c = scan_commit(&first, None);
        assert_eq!(c.persist_projects, ["a"]);
        assert!(c.next.peeked_projects.is_empty());
        let (_, diff) = scan_land(&c.next, &step("b", "", true, false));
        assert!(diff.collapse_projects.is_empty());
        assert_eq!(diff.expand_projects, ["b"]);
    }

    #[test]
    fn scan_commit_elsewhere_recollapses() {
        let (first, _) = scan_land(&ScanPeek::default(), &step("a", "", true, false));
        let c = scan_commit(&first, Some(("b", "")));
        assert!(c.persist_projects.is_empty());
        assert_eq!(c.collapse_projects, ["a"]);
        assert!(c.next.peeked_projects.is_empty());
        let (_, diff) = scan_land(&c.next, &step("b", "", true, false));
        assert!(diff.collapse_projects.is_empty());
    }

    #[test]
    fn scan_commit_is_stable() {
        let first = scan_commit(&ScanPeek::default(), Some(("a", "")));
        let again = scan_commit(&first.next, Some(("a", "")));
        assert_eq!(again.next, first.next);
    }

    #[test]
    fn scan_open_projects_are_not_peeked() {
        let (first, diff) = scan_land(&ScanPeek::default(), &step("a", "", false, false));
        assert!(diff.expand_projects.is_empty());
        assert!(first.peeked_projects.is_empty());
        let (_, diff) = scan_land(&first, &step("b", "", true, false));
        assert!(diff.collapse_projects.is_empty());
    }

    #[test]
    fn scan_peeks_groups() {
        let pay = step("payments", "g-api", true, true);
        let bill = step("billing", "g-api", true, false);
        let sf = step("storefront", "", false, false);
        let (into_pay, diff) = scan_land(&ScanPeek::default(), &pay);
        assert_eq!(diff.expand_groups, ["g-api"]);
        assert_eq!(diff.expand_projects, ["payments"]);
        let (into_bill, diff) = scan_land(&into_pay, &bill);
        assert_eq!(diff.collapse_projects, ["payments"]);
        assert!(diff.collapse_groups.is_empty());
        assert_eq!(diff.expand_projects, ["billing"]);
        assert_eq!(into_bill.peeked_groups, ["g-api"]);
        let (_, diff) = scan_land(&into_bill, &sf);
        assert_eq!(diff.collapse_projects, ["billing"]);
        assert_eq!(diff.collapse_groups, ["g-api"]);
    }

    #[test]
    fn scan_forget_drops_manual_toggle() {
        let (first, _) = scan_land(&ScanPeek::default(), &step("a", "", true, false));
        let forgotten = scan_forget(&first, Some("a"), None);
        let (_, diff) = scan_land(&forgotten, &step("b", "", true, false));
        assert!(diff.collapse_projects.is_empty());
    }

    #[test]
    fn reveal_delta() {
        assert_eq!(scroll_delta_to_reveal(80., 108., 0., 200., 30., 20.), 0.);
        assert_eq!(scroll_delta_to_reveal(10., 38., 0., 200., 30., 20.), -20.);
        assert_eq!(scroll_delta_to_reveal(170., 198., 0., 200., 30., 20.), 18.);
        assert_eq!(scroll_delta_to_reveal(0., 400., 0., 200., 30., 20.), -30.);
    }
}
