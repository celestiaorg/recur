//! `gh pr list` rows for the PR sweep, and per-PR state probes.

use std::path::Path;

use recur_domain::{Check, PrParticipant, PrReviewer};
use serde::Deserialize;
use serde_json::Value;

use super::GitHub;
use super::checks::bucket_from_state;
use super::thread::ListedActor;
use crate::json::{decode, decode_value};
use crate::model::{
    BUCKET_PASS, append_participant, first_non_empty, normalize_review_state, normalize_time,
    request_reviewer, sort_reviewers, upsert_reviewer,
};
use crate::{ForgeError, PrListRow, PrListState, PrStateProbe};

const LIST_FIELDS: &str = "number,title,url,headRefName,baseRefName,author,isDraft,isCrossRepository,createdAt,updatedAt,additions,deletions,changedFiles,reviewDecision,reviewRequests,assignees,latestReviews,state,mergeable,mergeStateStatus,statusCheckRollup";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ListedPr {
    pub number: i64,
    pub title: String,
    pub url: String,
    pub head_ref_name: String,
    pub base_ref_name: String,
    pub author: ListedActor,
    pub is_draft: bool,
    pub is_cross_repository: bool,
    pub created_at: String,
    pub updated_at: String,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub review_decision: String,
    pub review_requests: Vec<ListedActor>,
    pub assignees: Vec<ListedActor>,
    pub latest_reviews: Vec<ListedReview>,
    pub state: String,
    pub mergeable: String,
    pub merge_state_status: String,
    pub status_check_rollup: Option<Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ListedReview {
    pub author: ListedActor,
    pub state: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct CheckItem {
    name: String,
    context: String,
    conclusion: String,
    status: String,
    state: String,
}

/// Go `parseGitHubPRList`.
pub(crate) fn parse_list(raw: &str) -> Result<Vec<ListedPr>, String> {
    let mut raw = raw.trim();
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(i) = raw.find('[')
        && let Some(j) = raw.rfind(']')
        && j > i
    {
        raw = &raw[i..=j];
    }
    decode(raw).map_err(|e| format!("parse gh pr list json: {e}"))
}

fn check_items(row: &ListedPr) -> Vec<CheckItem> {
    let Some(v) = row.status_check_rollup.clone() else {
        return Vec::new();
    };
    if let Ok(items) = decode_value::<Vec<CheckItem>>(v.clone()) {
        return items;
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Wrap {
        nodes: Vec<CheckItem>,
    }
    decode_value::<Wrap>(v).map(|w| w.nodes).unwrap_or_default()
}

fn check_failed(conclusion: &str, state: &str) -> bool {
    [conclusion, state].iter().any(|v| {
        matches!(
            v.trim().to_lowercase().as_str(),
            "failure"
                | "error"
                | "cancelled"
                | "canceled"
                | "timed_out"
                | "action_required"
                | "failing"
                | "startup_failure"
        )
    })
}

fn listed_check(it: &CheckItem) -> Check {
    let mut name = it.name.trim();
    if name.is_empty() {
        name = it.context.trim();
    }
    if name.is_empty() {
        name = "check";
    }
    let status = it.status.trim();
    let mut bucket = None;
    if status.eq_ignore_ascii_case("COMPLETED") || status.is_empty() {
        let c = it.conclusion.trim();
        if !c.is_empty() {
            bucket = Some(bucket_from_state(c));
        }
    }
    let bucket = bucket.unwrap_or_else(|| bucket_from_state(first_non_empty(status, &it.state)));
    Check {
        name: name.to_owned(),
        passed: bucket == BUCKET_PASS,
        status: bucket.to_owned(),
        ..Default::default()
    }
}

/// Go `listedChecks`: rows, nothing failed, any reported.
fn listed_checks(row: &ListedPr) -> (Vec<Check>, bool, bool) {
    let items = check_items(row);
    if items.is_empty() {
        return (Vec::new(), true, false);
    }
    let ok = !items
        .iter()
        .any(|it| check_failed(&it.conclusion, &it.state));
    (items.iter().map(listed_check).collect(), ok, true)
}

/// Go `listedParticipants`: author, assignees, requested reviewers, reviewers.
fn listed_participants(row: &ListedPr) -> Vec<PrParticipant> {
    let mut out = Vec::new();
    append_participant(&mut out, row.author.participant());
    for a in row.assignees.iter().chain(&row.review_requests) {
        append_participant(&mut out, a.participant());
    }
    for r in &row.latest_reviews {
        append_participant(&mut out, r.author.participant());
    }
    out
}

/// Go `listedReviewers`.
fn listed_reviewers(row: &ListedPr) -> Vec<PrReviewer> {
    let mut out = Vec::new();
    for r in &row.latest_reviews {
        let st = normalize_review_state(&r.state);
        if !st.is_empty() {
            upsert_reviewer(
                &mut out,
                crate::model::actor_reviewer(&r.author.participant(), st),
            );
        }
    }
    for a in &row.review_requests {
        request_reviewer(&mut out, &a.participant());
    }
    sort_reviewers(out)
}

impl ListedPr {
    pub(crate) fn into_row(self) -> PrListRow {
        let (checks, checks_ok, has_checks) = listed_checks(&self);
        let participants = listed_participants(&self);
        let reviewers = listed_reviewers(&self);
        PrListRow {
            number: self.number,
            title: self.title,
            url: self.url,
            head_ref: self.head_ref_name,
            base_ref: self.base_ref_name,
            author_login: self.author.login.trim().to_owned(),
            is_draft: self.is_draft,
            is_cross_repository: self.is_cross_repository,
            created_at: self.created_at,
            updated_at: self.updated_at,
            additions: self.additions,
            deletions: self.deletions,
            changed_files: self.changed_files,
            review_decision: self.review_decision,
            state: self.state,
            mergeable: self.mergeable,
            merge_state_status: self.merge_state_status,
            review_requests: self
                .review_requests
                .iter()
                .map(ListedActor::participant)
                .collect(),
            assignees: self
                .assignees
                .iter()
                .map(ListedActor::participant)
                .collect(),
            checks,
            checks_ok,
            has_checks,
            participants,
            reviewers,
        }
    }
}

/// `gh pr list` argv. Merged rows search by update time so a long-lived PR
/// that just merged stays inside the cap (the default orders by creation).
pub(crate) fn list_args(state: PrListState, limit: usize) -> Vec<String> {
    let limit = if limit == 0 {
        PrListState::Open.sweep_limit()
    } else {
        limit
    };
    let mut args = vec![
        "pr".to_owned(),
        "list".to_owned(),
        "--limit".to_owned(),
        limit.to_string(),
        "--json".to_owned(),
        LIST_FIELDS.to_owned(),
    ];
    match state {
        PrListState::Merged => {
            args.extend(["--state", "merged", "--search", "sort:updated-desc"].map(str::to_owned))
        }
        PrListState::Open => args.extend(["--state", "open"].map(str::to_owned)),
    }
    args
}

/// Go `fetchGitHubPRViewState` answer parsing.
pub(crate) fn parse_view_state(raw: &str) -> Result<PrStateProbe, String> {
    let mut raw = raw.trim();
    if let Some(i) = raw.find('{')
        && let Some(j) = raw.rfind('}')
        && j > i
    {
        raw = &raw[i..=j];
    }
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Row {
        state: String,
        updated_at: String,
    }
    let row: Row = decode(raw).map_err(|e| format!("parse gh pr view state: {e}"))?;
    Ok(PrStateProbe {
        state: row.state.trim().to_uppercase(),
        updated_at: normalize_time(&row.updated_at),
    })
}

impl GitHub {
    /// Go `fetchGitHubPRList`.
    pub(crate) async fn list(
        &self,
        repo: &Path,
        state: PrListState,
        limit: usize,
    ) -> Result<Vec<PrListRow>, ForgeError> {
        if repo.as_os_str().is_empty() || repo.to_string_lossy().trim().is_empty() {
            return Err(ForgeError::invalid("empty repo path"));
        }
        let out = self.gh.run(repo, &list_args(state, limit)).await?;
        let rows = parse_list(&out).map_err(ForgeError::failed)?;
        Ok(rows.into_iter().map(ListedPr::into_row).collect())
    }

    /// Go `fetchGitHubPRViewState`.
    pub(crate) async fn view_state(
        &self,
        repo: &Path,
        number: i64,
    ) -> Result<PrStateProbe, ForgeError> {
        if number <= 0 {
            return Err(ForgeError::invalid(format!("invalid PR number {number}")));
        }
        let out = self
            .gh
            .run(
                repo,
                &[
                    "pr".to_owned(),
                    "view".to_owned(),
                    number.to_string(),
                    "--json".to_owned(),
                    "state,updatedAt".to_owned(),
                ],
            )
            .await?;
        parse_view_state(&out).map_err(ForgeError::failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use recur_domain::{
        REVIEW_STATE_APPROVED, REVIEW_STATE_CHANGES_REQUESTED, REVIEW_STATE_PENDING,
    };

    fn actor(login: &str) -> ListedActor {
        ListedActor {
            login: login.into(),
            avatar_url: String::new(),
        }
    }

    fn review(login: &str, state: &str) -> ListedReview {
        ListedReview {
            author: actor(login),
            state: state.into(),
        }
    }

    #[test]
    fn participants_order_and_dedupe() {
        let row = ListedPr {
            author: actor("sam"),
            assignees: vec![actor("pat")],
            review_requests: vec![actor("avery"), actor("sam")],
            latest_reviews: vec![
                review("avery", ""),
                review("copilot-pull-request-reviewer", ""),
            ],
            ..Default::default()
        };
        let got = listed_participants(&row);
        let logins: Vec<_> = got.iter().map(|p| p.login.as_str()).collect();
        assert_eq!(
            logins,
            ["sam", "pat", "avery", "copilot-pull-request-reviewer"]
        );
        assert!(got.iter().all(|p| !p.avatar_url.is_empty()));
    }

    #[test]
    fn reviewers_pending_and_approved() {
        let row = ListedPr {
            review_requests: vec![actor("avery"), actor("pat")],
            latest_reviews: vec![
                review("sam", "APPROVED"),
                review("pat", "COMMENTED"),
                review("lee", "CHANGES_REQUESTED"),
                review("ghost", "DISMISSED"),
            ],
            ..Default::default()
        };
        let got: Vec<String> = listed_reviewers(&row)
            .iter()
            .map(|r| format!("{}:{}", r.login, r.state))
            .collect();
        assert_eq!(
            got,
            [
                format!("lee:{REVIEW_STATE_CHANGES_REQUESTED}"),
                format!("avery:{REVIEW_STATE_PENDING}"),
                format!("pat:{REVIEW_STATE_PENDING}"),
                format!("sam:{REVIEW_STATE_APPROVED}"),
            ]
        );
        let row = ListedPr {
            review_requests: vec![actor("sam")],
            latest_reviews: vec![review("sam", "APPROVED")],
            ..Default::default()
        };
        let got = listed_reviewers(&row);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].state, REVIEW_STATE_APPROVED);
    }

    fn rows(raw: &str) -> Vec<PrListRow> {
        parse_list(raw)
            .unwrap()
            .into_iter()
            .map(ListedPr::into_row)
            .collect()
    }

    #[test]
    fn parse_github_pr_list() {
        use crate::model::{GROUP_OPEN, GROUP_YOU, listed_attention_reasons, listed_state_group};
        let rows = rows(
            r#"[{"number": 12, "title": "Add OAuth", "url": "https://github.com/acme/app/pull/12",
             "headRefName": "feat/oauth", "baseRefName": "main",
             "author": {"login": "sam"}, "isDraft": false, "createdAt": "2026-08-01T09:00:00Z", "updatedAt": "2026-08-21T12:00:00Z",
             "additions": 10, "deletions": 2, "changedFiles": 3, "reviewDecision": "REVIEW_REQUIRED",
             "reviewRequests": [{"login": "avery"}], "assignees": [], "state": "OPEN"}]"#,
        );
        let r = &rows[0];
        assert_eq!((r.number, r.author_login.as_str()), (12, "sam"));
        assert_eq!(r.created_at, "2026-08-01T09:00:00Z");
        assert_eq!(
            listed_attention_reasons(r, "avery"),
            [recur_domain::ATTENTION_REVIEW_REQUESTED]
        );
        assert!(listed_attention_reasons(r, "sam").is_empty());
        assert_eq!(
            listed_state_group(r, "avery"),
            ("Review required", GROUP_YOU)
        );
        assert_eq!(
            listed_state_group(r, "other"),
            ("Review required", GROUP_OPEN)
        );
    }

    #[test]
    fn listed_checks_ok_reads_conclusion_not_name() {
        let ok = |raw: &str| {
            let r = &rows(&format!(r#"[{{"number":1,"statusCheckRollup":{raw}}}]"#))[0];
            (r.checks_ok, r.has_checks)
        };
        assert_eq!(
            ok(r#"[{"name":"Error","conclusion":"success"}]"#),
            (true, true)
        );
        assert_eq!(ok(r#"[{"conclusion":"FAILURE"}]"#), (false, true));
        assert_eq!(ok("null"), (true, false));
        for pending in [
            r#"[{"name":"build","status":"IN_PROGRESS","conclusion":null}]"#,
            r#"[{"context":"license","state":"PENDING"}]"#,
            r#"[{"name":"lint","status":"QUEUED"}]"#,
        ] {
            assert_eq!(ok(pending), (true, true), "{pending}");
        }
        let fail = &rows(
            r#"[{"number":1,"statusCheckRollup":[{"name":"build","conclusion":"FAILURE"}]}]"#,
        )[0];
        assert_eq!(
            (
                fail.checks.len(),
                fail.checks[0].status.as_str(),
                fail.checks[0].passed
            ),
            (1, "fail", false)
        );
    }

    #[test]
    fn rollup_checks_from_array_or_nodes() {
        let rows = parse_list(
            r#"noise [{"number":1,"statusCheckRollup":[
                {"name":"build","status":"COMPLETED","conclusion":"SUCCESS"},
                {"context":"ci/x","state":"PENDING"},
                {"name":"lint","status":"COMPLETED","conclusion":"FAILURE"}
            ]},{"number":2,"statusCheckRollup":{"nodes":[{"name":"t","status":"IN_PROGRESS"}]}},
            {"number":3,"statusCheckRollup":null}] trailer"#,
        )
        .unwrap();
        let rows: Vec<PrListRow> = rows.into_iter().map(ListedPr::into_row).collect();
        let st: Vec<(&str, &str, bool)> = rows[0]
            .checks
            .iter()
            .map(|c| (c.name.as_str(), c.status.as_str(), c.passed))
            .collect();
        assert_eq!(
            st,
            [
                ("build", "pass", true),
                ("ci/x", "pending", false),
                ("lint", "fail", false)
            ]
        );
        assert!(!rows[0].checks_ok && rows[0].has_checks);
        assert_eq!(rows[1].checks[0].status, "pending");
        assert!(rows[1].checks_ok);
        assert!(rows[2].checks.is_empty() && rows[2].checks_ok && !rows[2].has_checks);
        assert!(parse_list("").unwrap().is_empty());
        assert!(parse_list("[{").is_err());
    }

    #[test]
    fn list_args_merged_sorts_by_update() {
        assert_eq!(
            list_args(PrListState::Merged, 20)[7..],
            ["merged", "--search", "sort:updated-desc"]
        );
        assert_eq!(list_args(PrListState::Open, 0)[3], "100");
    }

    #[test]
    fn view_state_normalizes() {
        assert_eq!(
            parse_view_state(r#"x{"state":"merged","updatedAt":"2026-08-21T12:00:00.5+02:00"}y"#)
                .unwrap(),
            PrStateProbe {
                state: "MERGED".into(),
                updated_at: "2026-08-21T10:00:00Z".into()
            }
        );
        assert!(parse_view_state("nope").is_err());
    }
}
