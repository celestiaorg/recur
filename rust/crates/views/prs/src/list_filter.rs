//! `lib/pr-list-filter.ts`: the PR dashboard's lifecycle segments, status facets, sort orders,
//! groups and titlebar tallies.

use std::cmp::Ordering;

use chrono::DateTime;
use recur_domain::{ATTENTION_ASSIGNED, ATTENTION_CHANGES_REQUESTED, Pr};
use recur_forge::view::{RollupKind, pr_checks_rollup, pr_has_review_request};
use recur_ui::IconName;
use recur_view_core::pr::{PrLifecycle, pr_lifecycle};

/// The lifecycle segment (`PRListLife`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListLife {
    #[default]
    Open,
    Draft,
    Merged,
    Closed,
    All,
}

/// `PR_LIST_SEGS`.
pub const LIST_SEGS: [(ListLife, &str); 5] = [
    (ListLife::Open, "Open"),
    (ListLife::Draft, "Draft"),
    (ListLife::Merged, "Merged"),
    (ListLife::Closed, "Closed"),
    (ListLife::All, "All"),
];

/// `PR_LIST_FACETS`: value, label, group.
pub const LIST_FACETS: [(&str, &str, &str); 6] = [
    ("review_requested", "Needs your review", "Review"),
    ("changes_requested", "Changes requested", "Review"),
    ("assigned", "Assigned to you", "Review"),
    ("in_review", "In review", "Review"),
    ("checks_failing", "Checks failing", "Checks"),
    ("checks_passing", "Checks passing", "Checks"),
];

/// `PRListSort`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ListSort {
    #[default]
    Updated,
    UpdatedAsc,
    Newest,
    Oldest,
    Title,
}

impl ListSort {
    pub fn id(self) -> &'static str {
        match self {
            ListSort::Updated => "updated",
            ListSort::UpdatedAsc => "updated-asc",
            ListSort::Newest => "newest",
            ListSort::Oldest => "oldest",
            ListSort::Title => "title",
        }
    }

    pub fn from_id(id: &str) -> Option<ListSort> {
        LIST_SORTS.iter().map(|s| s.0).find(|s| s.id() == id)
    }
}

/// `PR_LIST_SORTS`: sort, label, icon.
pub const LIST_SORTS: [(ListSort, &str, IconName); 5] = [
    (
        ListSort::Updated,
        "Recently updated",
        IconName::ClockArrowDown,
    ),
    (
        ListSort::UpdatedAsc,
        "Least recently updated",
        IconName::ClockArrowUp,
    ),
    (ListSort::Newest, "Newest", IconName::CalendarArrowDown),
    (ListSort::Oldest, "Oldest", IconName::CalendarArrowUp),
    (ListSort::Title, "Title A–Z", IconName::ArrowUpAz),
];

/// Queued PRs are still open on GitHub, so they list and count as open.
pub fn list_lifecycle(pr: Option<&Pr>) -> Option<PrLifecycle> {
    match pr_lifecycle(pr) {
        Some(PrLifecycle::Queued) => Some(PrLifecycle::Open),
        life => life,
    }
}

pub fn matches_life(pr: Option<&Pr>, life: ListLife) -> bool {
    let Some(got) = list_lifecycle(pr) else {
        return life == ListLife::All;
    };
    match life {
        ListLife::All => true,
        ListLife::Open => got == PrLifecycle::Open,
        ListLife::Draft => got == PrLifecycle::Draft,
        ListLife::Merged => got == PrLifecycle::Merged,
        ListLife::Closed => got == PrLifecycle::Closed,
    }
}

pub fn matches_facet(pr: &Pr, facet: &str) -> bool {
    let state = pr.state.to_lowercase();
    match facet {
        "review_requested" => pr_has_review_request(pr),
        "changes_requested" => {
            pr.attention_reasons
                .iter()
                .any(|c| c == ATTENTION_CHANGES_REQUESTED)
                || state.contains("changes requested")
        }
        "assigned" => pr.attention_reasons.iter().any(|c| c == ATTENTION_ASSIGNED),
        "in_review" => state.contains("in review") || state.contains("review required"),
        "checks_failing" => pr_checks_rollup(pr) == RollupKind::Fail,
        "checks_passing" => pr_checks_rollup(pr) == RollupKind::Pass,
        _ => false,
    }
}

/// Lifecycle first, then every selected status (AND). Empty facets = no extra cut.
pub fn matches_list_filter(pr: &Pr, life: ListLife, facets: &[String]) -> bool {
    matches_life(Some(pr), life) && facets.iter().all(|f| matches_facet(pr, f))
}

/// `PR_LIST_GROUP_ORDER`.
pub const GROUP_ORDER: [&str; 4] = ["you", "loop", "open", "merged"];

/// Milliseconds since the epoch of an RFC3339 stamp; missing or invalid sorts as oldest.
pub fn created_ms(stamp: &str) -> f64 {
    let s = stamp.trim();
    if s.is_empty() {
        return f64::NEG_INFINITY;
    }
    DateTime::parse_from_rfc3339(s).map_or(f64::NEG_INFINITY, |t| t.timestamp_millis() as f64)
}

/// Last-activity time: `updatedAt`, then `createdAt`. `when` is display-only.
pub fn updated_ms(pr: &Pr) -> f64 {
    let u = created_ms(&pr.updated_at);
    if u != f64::NEG_INFINITY {
        return u;
    }
    created_ms(&pr.created_at)
}

fn cmp_ms(a: f64, b: f64) -> Ordering {
    // `a - b` in JS: two -Infinity stamps compare as NaN, which sort treats as 0.
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

fn tie(a: &Pr, b: &Pr, created_desc: bool) -> Ordering {
    let d = if created_desc {
        cmp_ms(created_ms(&b.created_at), created_ms(&a.created_at))
    } else {
        cmp_ms(created_ms(&a.created_at), created_ms(&b.created_at))
    };
    d.then_with(|| a.id.cmp(&b.id))
}

/// `localeCompare(…, { sensitivity: 'base' })` for titles: case- and accent-insensitive.
fn title_cmp(a: &str, b: &str) -> Ordering {
    let key = |s: &str| -> Vec<char> {
        s.chars()
            .flat_map(char::to_lowercase)
            .map(fold_accent)
            .collect()
    };
    key(a).cmp(&key(b))
}

fn fold_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        c => c,
    }
}

pub fn compare(a: &Pr, b: &Pr, sort: ListSort) -> Ordering {
    match sort {
        ListSort::UpdatedAsc => cmp_ms(updated_ms(a), updated_ms(b)).then_with(|| tie(a, b, false)),
        ListSort::Newest => cmp_ms(created_ms(&b.created_at), created_ms(&a.created_at))
            .then_with(|| a.id.cmp(&b.id)),
        ListSort::Oldest => cmp_ms(created_ms(&a.created_at), created_ms(&b.created_at))
            .then_with(|| a.id.cmp(&b.id)),
        ListSort::Title => title_cmp(&a.title, &b.title).then_with(|| tie(a, b, true)),
        ListSort::Updated => cmp_ms(updated_ms(b), updated_ms(a)).then_with(|| tie(a, b, true)),
    }
}

/// Copy + sort (stable). Default is recently updated, then later `createdAt`.
pub fn sort_list(list: &[Pr], sort: ListSort) -> Vec<Pr> {
    let mut out = list.to_vec();
    out.sort_by(|a, b| compare(a, b, sort));
    out
}

/// Drafts are not a needs-you inbox, even if GitHub still has a stale you-group.
pub fn list_group(pr: &Pr) -> String {
    let g = if pr.group.is_empty() {
        "open"
    } else {
        pr.group.trim()
    };
    let g = if g.is_empty() { "open" } else { g };
    if g == "you" && (pr.draft || pr.merge_state_status.eq_ignore_ascii_case("draft")) {
        return "open".into();
    }
    g.to_string()
}

/// Titlebar tallies: the live inbox, not Recur's you / loop / open buckets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TitleCounts {
    pub you: usize,
    pub open: usize,
    pub merged: usize,
}

pub fn title_counts<'a>(prs: impl IntoIterator<Item = &'a Pr>) -> TitleCounts {
    let mut c = TitleCounts::default();
    for p in prs {
        match list_lifecycle(Some(p)) {
            Some(PrLifecycle::Merged) => c.merged += 1,
            Some(PrLifecycle::Open | PrLifecycle::Draft) => {
                c.open += 1;
                if list_group(p) == "you" {
                    c.you += 1;
                }
            }
            _ => {}
        }
    }
    c
}

/// `scopedPRs`: PRs visible under the project filter (empty selection = all). Legacy rows with
/// an empty `projectId` resolve through their workspace.
pub fn scoped_prs<'a>(
    list: &'a [Pr],
    project_ids: &[String],
    workspace_project: impl Fn(&str) -> Option<&'a str>,
) -> Vec<&'a Pr> {
    if project_ids.is_empty() {
        return list.iter().collect();
    }
    list.iter()
        .filter(|p| {
            let mut pid = p.project_id.trim();
            if pid.is_empty() && !p.workspace_id.is_empty() {
                pid = workspace_project(&p.workspace_id)
                    .unwrap_or_default()
                    .trim();
            }
            project_ids.iter().any(|id| id == pid)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use recur_domain::{Check, PrSource};

    use super::*;

    fn open_review() -> Pr {
        Pr {
            group: "you".into(),
            state: "In review".into(),
            review_requested: true,
            checks_ok: true,
            has_checks: true,
            // `{ name: 'test' }` in the TS fixture is skipped by `prChecksRollup`; Go always
            // sends `passed`, so the equivalent row here is a passed check.
            checks: Some(vec![Check {
                name: "test".into(),
                passed: true,
                ..Default::default()
            }]),
            ..Default::default()
        }
    }

    fn pr(f: impl FnOnce(&mut Pr)) -> Pr {
        let mut p = Pr {
            checks_ok: true,
            ..Default::default()
        };
        f(&mut p);
        p
    }

    #[test]
    fn open_hides_drafts_merged_and_closed() {
        let o = open_review();
        assert!(matches_life(Some(&o), ListLife::Open));
        let queued = Pr {
            merge_queued: true,
            ..open_review()
        };
        assert!(matches_life(Some(&queued), ListLife::Open));
        let draft = Pr {
            draft: true,
            ..open_review()
        };
        assert!(!matches_life(Some(&draft), ListLife::Open));
        let merged = pr(|p| {
            p.group = "merged".into();
            p.state = "Merged".into();
        });
        assert!(!matches_life(Some(&merged), ListLife::Open));
        let closed = pr(|p| p.state = "Closed".into());
        assert!(!matches_life(Some(&closed), ListLife::Open));
        let d = pr(|p| {
            p.draft = true;
            p.state = "In review".into();
        });
        assert!(matches_life(Some(&d), ListLife::Draft));
        assert!(matches_life(Some(&closed), ListLife::Closed));
        assert!(matches_life(Some(&merged), ListLife::All));
        assert!(matches_life(None, ListLife::All));
        assert!(!matches_life(None, ListLife::Open));
    }

    #[test]
    fn review_and_check_facets_read_the_stored_flags() {
        let o = open_review();
        assert!(matches_facet(&o, "review_requested"));
        assert!(matches_facet(&o, "in_review"));
        assert!(matches_facet(&o, "checks_passing"));
        assert!(!matches_facet(&o, "checks_failing"));
        assert!(matches_facet(
            &pr(|p| p.state = "Changes requested".into()),
            "changes_requested"
        ));
        assert!(matches_facet(
            &pr(|p| p.attention_reasons = vec!["assigned".into()]),
            "assigned"
        ));
        assert!(!matches_facet(&o, "bogus"));
    }

    #[test]
    fn pending_github_checks_are_not_failing() {
        let pending = Pr {
            checks_ok: false,
            checks: Some(vec![Check {
                name: "build".into(),
                passed: false,
                status: "pending".into(),
                ..Default::default()
            }]),
            ..open_review()
        };
        assert!(!matches_facet(&pending, "checks_failing"));
        assert!(!matches_facet(&pending, "checks_passing"));
        assert!(!matches_list_filter(
            &pending,
            ListLife::Open,
            &["checks_failing".into()]
        ));
    }

    #[test]
    fn ands_status_facets_on_top_of_the_lifecycle_cut() {
        let o = open_review();
        let f = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(matches_list_filter(
            &o,
            ListLife::Open,
            &f(&["review_requested", "checks_passing"])
        ));
        assert!(!matches_list_filter(
            &o,
            ListLife::Open,
            &f(&["checks_failing"])
        ));
        let draft = Pr {
            draft: true,
            ..open_review()
        };
        assert!(!matches_list_filter(
            &draft,
            ListLife::Open,
            &f(&["review_requested"])
        ));
        assert!(matches_list_filter(
            &draft,
            ListLife::Draft,
            &f(&["in_review"])
        ));
    }

    #[test]
    fn counts_live_prs_and_omits_loop_as_its_own_tally() {
        let list = [
            open_review(),
            pr(|p| {
                p.group = "loop".into();
                p.state = "Loop responding".into();
            }),
            pr(|p| {
                p.group = "merged".into();
                p.state = "Merged".into();
            }),
            pr(|p| {
                p.group = "you".into();
                p.draft = true;
                p.state = "In review".into();
            }),
            pr(|p| p.state = "Closed".into()),
        ];
        assert_eq!(
            title_counts(&list),
            TitleCounts {
                you: 1,
                open: 3,
                merged: 1
            }
        );
    }

    #[test]
    fn moves_github_drafts_out_of_needs_your_attention() {
        assert_eq!(
            list_group(&pr(|p| {
                p.group = "you".into();
                p.draft = true;
            })),
            "open"
        );
        assert_eq!(
            list_group(&pr(|p| {
                p.group = "you".into();
                p.review_requested = true;
            })),
            "you"
        );
        assert_eq!(
            list_group(&pr(|p| {
                p.group = "loop".into();
                p.draft = true;
            })),
            "loop"
        );
        assert_eq!(list_group(&pr(|_| {})), "open");
        assert_eq!(list_group(&pr(|p| p.group = "  ".into())), "open");
    }

    #[test]
    fn each_sort_option_has_a_registered_icon() {
        assert_eq!(
            LIST_SORTS.map(|s| s.2.key()),
            [
                "clock-arrow-down",
                "clock-arrow-up",
                "calendar-arrow-down",
                "calendar-arrow-up",
                "arrow-up-az"
            ]
        );
        for s in LIST_SORTS {
            assert_eq!(IconName::from_key(s.2.key()), Some(s.2));
            assert_eq!(ListSort::from_id(s.0.id()), Some(s.0));
        }
    }

    #[test]
    fn parses_rfc3339_and_treats_blanks_as_oldest() {
        assert_eq!(created_ms("2026-08-28T12:00:00Z"), 1_787_918_400_000.);
        assert_eq!(created_ms(""), f64::NEG_INFINITY);
        assert_eq!(created_ms("nope"), f64::NEG_INFINITY);
    }

    fn row(id: &str, title: &str, created: &str, updated: &str) -> Pr {
        Pr {
            id: id.into(),
            title: title.into(),
            created_at: created.into(),
            updated_at: updated.into(),
            source: PrSource::GitHub,
            ..Default::default()
        }
    }

    fn ids(list: Vec<Pr>) -> Vec<String> {
        list.into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn sorts() {
        let zebra = row("a", "Zebra", "2026-08-28T12:00:00Z", "2026-08-26T12:00:00Z");
        let alpha = row("b", "Alpha", "2026-08-01T12:00:00Z", "2026-08-28T11:00:00Z");
        let mid = row("c", "mid", "2026-08-15T12:00:00Z", "2026-08-28T11:00:00Z");
        let all = [zebra.clone(), alpha.clone(), mid.clone()];
        // Default is recently updatedAt, not a frozen when=now.
        assert_eq!(ids(sort_list(&all, ListSort::Updated)), ["c", "b", "a"]);
        assert_eq!(compare(&alpha, &zebra, ListSort::Updated), Ordering::Less);
        assert!(updated_ms(&alpha) > updated_ms(&zebra));
        // Least recently updated flips updatedAt and ties by older createdAt.
        assert_eq!(
            ids(sort_list(
                &[alpha.clone(), zebra.clone()],
                ListSort::UpdatedAsc
            )),
            ["a", "b"]
        );
        assert_eq!(
            ids(sort_list(
                &[alpha.clone(), mid.clone()],
                ListSort::UpdatedAsc
            )),
            ["b", "c"]
        );
        // Newest / oldest use createdAt, not the PR number.
        assert_eq!(ids(sort_list(&all, ListSort::Newest)), ["a", "c", "b"]);
        assert_eq!(ids(sort_list(&all, ListSort::Oldest)), ["b", "c", "a"]);
        // Title is case-insensitive A–Z.
        let titles: Vec<String> = sort_list(&all, ListSort::Title)
            .into_iter()
            .map(|p| p.title)
            .collect();
        assert_eq!(titles, ["Alpha", "mid", "Zebra"]);
        // Does not mutate the input.
        let list = [zebra, alpha];
        let _ = sort_list(&list, ListSort::Oldest);
        assert_eq!(list.map(|p| p.id), ["a", "b"]);
    }

    #[test]
    fn scoped_prs_resolve_legacy_rows_through_their_workspace() {
        let list = [
            pr(|p| {
                p.id = "1".into();
                p.project_id = "a".into();
            }),
            pr(|p| {
                p.id = "2".into();
                p.workspace_id = "w".into();
            }),
            pr(|p| {
                p.id = "3".into();
                p.project_id = "b".into();
            }),
        ];
        let ws = |id: &str| (id == "w").then_some("a");
        let got: Vec<&str> = scoped_prs(&list, &["a".into()], ws)
            .into_iter()
            .map(|p| p.id.as_str())
            .collect();
        assert_eq!(got, ["1", "2"]);
        assert_eq!(scoped_prs(&list, &[], ws).len(), 3);
    }
}
