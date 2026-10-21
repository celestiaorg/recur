//! One GraphQL query for the PR view, threads, commit rollup, reactions and
//! viewer, plus the REST timeline, combined into a [`PrSnapshot`].

use std::collections::HashMap;
use std::path::Path;

use chrono::{DateTime, Utc};
use recur_domain::MergeMethod;
use serde::Deserialize;

use super::GitHub;
use super::reactions::{apply_reactions, parse_reactions};
use super::thread::{
    Actor, GhSnapshot, InlineComment, ListedActor, ViewComment, ViewCommit, ViewCommitAuthor,
    ViewJson, ViewReview, apply_avatars, apply_review_threads, attach_review_comments,
    comments_from_inline_rows, parse_commit_rollup, parse_review_threads, parse_view_json,
    remember_avatar, sort_comments,
};
use super::timeline::parse_timeline;
use crate::json::decode;
use crate::{ForgeError, PrSnapshot, Viewer};

/// Combined GraphQL snapshot for PR view + threads + rollup + reactions +
/// viewer. The timeline stays on REST: GraphQL `timelineItems` drops Copilot
/// work events.
pub const SNAPSHOT_QUERY: &str = r#"query($o:String!,$r:String!,$n:Int!){
  rateLimit { cost remaining nodeCount }
  viewer { login }
  repository(owner:$o,name:$r){
    viewerPermission
    mergeCommitAllowed
    squashMergeAllowed
    rebaseMergeAllowed
    pullRequest(number:$n){
      id title body state reviewDecision mergeable mergeStateStatus isDraft isMergeQueueEnabled isInMergeQueue autoMergeRequest { enabledAt }
      url headRefOid headRefName baseRefName createdAt updatedAt
      author { login avatarUrl }
      reactionGroups { content viewerHasReacted reactors { totalCount } }
      reviewRequests(first:20){ nodes { requestedReviewer { __typename ... on User { login avatarUrl } ... on Bot { login avatarUrl } ... on Team { name } } } }
      assignees(first:20){ nodes { login avatarUrl } }
      comments(last:100){ pageInfo { hasPreviousPage } nodes { id databaseId url author { login avatarUrl } body createdAt reactionGroups { content viewerHasReacted reactors { totalCount } } } }
      reviews(last:100){ pageInfo { hasPreviousPage } nodes { id databaseId url author { login avatarUrl } body state submittedAt reactionGroups { content viewerHasReacted reactors { totalCount } } } }
      commits(last:100){ nodes { commit { oid url messageHeadline messageBody committedDate signature { isValid } statusCheckRollup { state } authors(first:8){ nodes { name user { login avatarUrl } } } } } }
      reviewThreads(first:100){ nodes { id isResolved diffSide startDiffSide comments(first:50){ nodes { id databaseId url author { login avatarUrl } body createdAt path originalLine line originalStartLine startLine outdated diffHunk replyTo { databaseId } pullRequestReview { databaseId } reactionGroups { content viewerHasReacted reactors { totalCount } } } } } }
    }
  }
}"#;

/// The snapshot query, without `statusCheckRollup` when the token may lack
/// checks permission (a PAT/App that can read the PR still gets the rest).
pub fn snapshot_query(include_check_rollup: bool) -> String {
    if include_check_rollup {
        SNAPSHOT_QUERY.to_owned()
    } else {
        SNAPSHOT_QUERY.replacen("statusCheckRollup { state }", "", 1)
    }
}

/// `gh api` argv for the REST timeline (all pages).
pub(crate) fn timeline_args(owner: &str, repo: &str, number: i64) -> Vec<String> {
    vec![
        "api".into(),
        "--paginate".into(),
        "--slurp".into(),
        format!("repos/{owner}/{repo}/issues/{number}/timeline?per_page=100"),
        "-H".into(),
        "Accept: application/vnd.github+json".into(),
    ]
}

/// `gh api --paginate --slurp` output (an array of page arrays) as one JSON
/// array. A flat array, or anything else, is returned as is.
pub(crate) fn flatten_json_array_pages(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let Ok(pages) = serde_json::from_str::<Vec<Vec<Box<serde_json::value::RawValue>>>>(raw) else {
        return raw.to_owned();
    };
    let all: Vec<&serde_json::value::RawValue> =
        pages.iter().flatten().map(AsRef::as_ref).collect();
    crate::json::marshal(&all)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Envelope {
    data: EnvData,
    errors: Vec<GqlError>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct GqlError {
    pub message: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EnvData {
    viewer: Login,
    repository: EnvRepo,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Login {
    login: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct EnvRepo {
    viewer_permission: String,
    merge_commit_allowed: bool,
    squash_merge_allowed: bool,
    rebase_merge_allowed: bool,
    pull_request: SnapPr,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct PageInfo {
    has_previous_page: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Paged<T> {
    page_info: PageInfo,
    nodes: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SnapPr {
    id: String,
    title: String,
    body: String,
    state: String,
    review_decision: String,
    mergeable: String,
    merge_state_status: String,
    is_draft: bool,
    #[serde(rename = "isMergeQueueEnabled")]
    is_merge_queue: bool,
    is_in_merge_queue: bool,
    #[serde(rename = "autoMergeRequest")]
    auto_merge: Option<serde::de::IgnoredAny>,
    head_ref_oid: String,
    head_ref_name: String,
    base_ref_name: String,
    url: String,
    created_at: String,
    updated_at: String,
    author: Actor,
    review_requests: Nodes<ReviewRequest>,
    assignees: Nodes<ListedActor>,
    comments: Paged<ViewComment>,
    reviews: Paged<ViewReview>,
    commits: Nodes<CommitEdge>,
    review_threads: Nodes<ThreadNode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ReviewRequest {
    requested_reviewer: Option<RequestedReviewer>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct RequestedReviewer {
    login: String,
    avatar_url: String,
    name: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CommitEdge {
    commit: SnapCommit,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SnapCommit {
    oid: String,
    message_headline: String,
    message_body: String,
    committed_date: String,
    authors: Nodes<CommitAuthorNode>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct CommitAuthorNode {
    name: String,
    user: Option<Login>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ThreadNode {
    diff_side: String,
    start_diff_side: String,
    comments: Nodes<ThreadComment>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ThreadComment {
    database_id: i64,
    url: String,
    author: Actor,
    body: String,
    created_at: String,
    path: String,
    original_line: i64,
    line: Option<i64>,
    original_start_line: i64,
    start_line: Option<i64>,
    diff_hunk: String,
    outdated: bool,
    reply_to: Option<DatabaseId>,
    pull_request_review: Option<DatabaseId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DatabaseId {
    database_id: i64,
}

/// Go `parseGitHubPRSnapshotGraphQL`.
pub(crate) fn parse_snapshot_graphql(raw: &str, now: DateTime<Utc>) -> Result<GhSnapshot, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty pull request graphql".into());
    }
    let env: Envelope = decode(raw).map_err(|e| format!("parse pull request graphql: {e}"))?;
    let repo = env.data.repository;
    let pr = repo.pull_request;
    if pr.id.trim().is_empty() && pr.url.trim().is_empty() {
        if let Some(e) = env.errors.first()
            && !e.message.trim().is_empty()
        {
            return Err(format!("pull request graphql: {}", e.message));
        }
        return Err("empty pull request graphql".into());
    }
    let comments_truncated = pr.comments.page_info.has_previous_page;
    let reviews_truncated = pr.reviews.page_info.has_previous_page;
    let inline = inline_comments(&pr);
    let merge_queue_enabled = pr.is_merge_queue;
    let merge_queued = pr.is_in_merge_queue || (pr.is_merge_queue && pr.auto_merge.is_some());
    let mut gh = parse_view_json(view_from_graphql(pr), now);
    let snap = &mut gh.snap;
    snap.comments_truncated = comments_truncated;
    snap.reviews_truncated = reviews_truncated;
    snap.comments
        .extend(comments_from_inline_rows(&inline, now, &snap.url));
    apply_review_threads(&mut snap.comments, &parse_review_threads(raw));
    let (meta, avatars) = parse_commit_rollup(raw);
    snap.commit_meta = meta;
    snap.author_avatars = avatars;
    if let Ok((hits, node_id)) = parse_reactions(raw) {
        apply_reactions(&mut snap.comments, &hits);
        if !node_id.is_empty() {
            snap.node_id = node_id;
        }
    }
    let viewer = viewer_from_snapshot(
        &env.data.viewer.login,
        &repo.viewer_permission,
        repo.merge_commit_allowed,
        repo.squash_merge_allowed,
        repo.rebase_merge_allowed,
    );
    snap.viewer_login = viewer.login;
    snap.viewer_can_write = viewer.can_write;
    snap.allowed_merge_methods = viewer.allowed_merge_methods;
    snap.merge_queue_enabled = merge_queue_enabled;
    snap.merge_queued = merge_queued;
    Ok(gh)
}

fn view_from_graphql(pr: SnapPr) -> ViewJson {
    ViewJson {
        id: pr.id,
        title: pr.title,
        body: pr.body,
        state: pr.state,
        review_decision: pr.review_decision,
        mergeable: pr.mergeable,
        merge_state_status: pr.merge_state_status,
        is_draft: pr.is_draft,
        head_ref_oid: pr.head_ref_oid,
        head_ref_name: pr.head_ref_name,
        base_ref_name: pr.base_ref_name,
        url: pr.url,
        created_at: pr.created_at,
        updated_at: pr.updated_at,
        author: pr.author,
        assignees: pr.assignees.nodes,
        comments: pr.comments.nodes,
        reviews: pr.reviews.nodes,
        commits: pr
            .commits
            .nodes
            .into_iter()
            .map(|n| ViewCommit {
                oid: n.commit.oid,
                message_headline: n.commit.message_headline,
                message_body: n.commit.message_body,
                committed_date: n.commit.committed_date,
                authors: n
                    .commit
                    .authors
                    .nodes
                    .into_iter()
                    .map(|a| ViewCommitAuthor {
                        login: a.user.map(|u| u.login).unwrap_or_default(),
                        name: a.name,
                    })
                    .collect(),
            })
            .collect(),
        review_requests: pr
            .review_requests
            .nodes
            .into_iter()
            .filter_map(|n| {
                let r = n.requested_reviewer?;
                let mut login = r.login.trim().to_owned();
                if login.is_empty() {
                    login = r.name.trim().to_owned();
                }
                (!login.is_empty()).then_some(ListedActor {
                    login,
                    avatar_url: r.avatar_url,
                })
            })
            .collect(),
    }
}

fn inline_comments(pr: &SnapPr) -> Vec<InlineComment> {
    let mut rows = Vec::new();
    for th in &pr.review_threads.nodes {
        for n in &th.comments.nodes {
            if n.database_id <= 0 {
                continue;
            }
            rows.push(InlineComment {
                id: n.database_id,
                html_url: n.url.clone(),
                body: n.body.clone(),
                path: n.path.clone(),
                line: if n.outdated { None } else { n.line },
                original_line: n.original_line,
                start_line: n.start_line,
                original_start_line: n.original_start_line,
                side: th.diff_side.clone(),
                start_side: th.start_diff_side.clone(),
                diff_hunk: n.diff_hunk.clone(),
                created_at: n.created_at.clone(),
                pull_request_review_id: n.pull_request_review.as_ref().map_or(0, |r| r.database_id),
                in_reply_to_id: n.reply_to.as_ref().map_or(0, |r| r.database_id),
                user: n.author.clone(),
            });
        }
    }
    rows
}

/// Viewer login, write access and merge methods from the snapshot fields.
pub(crate) fn viewer_from_snapshot(
    login: &str,
    perm: &str,
    merge: bool,
    squash: bool,
    rebase: bool,
) -> Viewer {
    Viewer {
        login: login.trim().to_owned(),
        can_write: permission_can_write(perm),
        allowed_merge_methods: allowed_merge_methods_from_flags(merge, squash, rebase),
    }
}

/// `WRITE`, `MAINTAIN` and `ADMIN` can push.
pub(crate) fn permission_can_write(perm: &str) -> bool {
    matches!(
        perm.trim().to_uppercase().as_str(),
        "WRITE" | "MAINTAIN" | "ADMIN"
    )
}

fn allowed_merge_methods_from_flags(merge: bool, squash: bool, rebase: bool) -> Vec<String> {
    [
        (merge, MergeMethod::Merge),
        (squash, MergeMethod::Squash),
        (rebase, MergeMethod::Rebase),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, m)| m.as_str().to_owned())
    .collect()
}

/// Go `finishGitHubPRSnapshot`: adds the timeline, nests reviews, upgrades
/// avatars and orders the thread.
pub(crate) fn finish(gh: GhSnapshot, timeline_raw: &str, now: DateTime<Utc>) -> PrSnapshot {
    let GhSnapshot {
        mut snap,
        commit_logins,
    } = gh;
    if !timeline_raw.trim().is_empty() {
        let rows = parse_timeline(
            timeline_raw,
            now,
            &commit_logins,
            &snap.commit_meta,
            &snap.url,
            &snap.head_ref,
            &snap.base_ref,
        );
        snap.comments.extend(rows);
    }
    attach_review_comments(&mut snap.comments);
    let mut avatars: HashMap<String, String> = std::mem::take(&mut snap.author_avatars);
    for c in &snap.comments {
        if c.avatar_url.contains("avatars.githubusercontent.com/") {
            let login = if c.author_login.is_empty() {
                &c.author
            } else {
                &c.author_login
            };
            remember_avatar(
                &mut avatars,
                &Actor {
                    login: login.clone(),
                    avatar_url: c.avatar_url.clone(),
                    ..Default::default()
                },
            );
        }
    }
    apply_avatars(&mut snap.comments, &avatars);
    sort_comments(&mut snap.comments);
    let login = snap.author_login.trim().to_owned();
    if !login.is_empty() {
        let provided = avatars
            .get(&login.to_lowercase())
            .cloned()
            .unwrap_or_default();
        snap.author_avatar_url = super::avatar_url(&login, &provided, 56);
    }
    snap.author_avatars = avatars;
    snap
}

impl GitHub {
    /// Go `fetchGitHubPRSnapshotGraphQL`: with the check rollup, retried
    /// without it when the token cannot read checks.
    async fn snapshot_graphql(
        &self,
        repo: &Path,
        owner: &str,
        name: &str,
        number: i64,
    ) -> Result<String, ForgeError> {
        let run = |q: String| async move {
            self.gh
                .run(
                    repo,
                    &[
                        "api".to_owned(),
                        "graphql".to_owned(),
                        "-f".to_owned(),
                        format!("query={q}"),
                        "-F".to_owned(),
                        format!("o={owner}"),
                        "-F".to_owned(),
                        format!("r={name}"),
                        "-F".to_owned(),
                        format!("n={number}"),
                    ],
                )
                .await
        };
        match run(snapshot_query(true)).await {
            Ok(out) => Ok(out),
            Err(err) => run(snapshot_query(false)).await.map_err(|_| err.into()),
        }
    }

    /// Go `fetchGitHubPRSnapshot`: one GraphQL query plus the REST timeline,
    /// in parallel.
    pub(crate) async fn fetch_snapshot(
        &self,
        repo: &Path,
        number: i64,
    ) -> Result<PrSnapshot, ForgeError> {
        if number <= 0 {
            return Err(ForgeError::invalid(format!(
                "invalid GitHub PR number {number}"
            )));
        }
        let (owner, name) = self.repo_owner_name(repo).await?;
        let timeline = timeline_args(&owner, &name, number);
        let (gql, tl) = tokio::join!(
            self.snapshot_graphql(repo, &owner, &name, number),
            self.gh.run(repo, &timeline)
        );
        let gql = gql?;
        let now = Utc::now();
        let gh = parse_snapshot_graphql(&gql, now).map_err(ForgeError::failed)?;
        Ok(match tl {
            Ok(out) => finish(gh, &flatten_json_array_pages(&out), now),
            Err(_) => finish(gh, "", now),
        })
    }
}
