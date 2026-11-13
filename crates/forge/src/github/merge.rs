//! Merging GitHub PRs: REST merge (retried while mergeability is pending),
//! GraphQL merge, `gh pr merge`, and the merge queue. Plus merge-commit text.

use std::path::Path;
use std::time::Duration;

use recur_domain::{
    Commit, MergeCommitMessage, MergeMethod, Pr, PrComment, SquashCommitMessage,
    normalize_merge_commit_message, normalize_merge_method, normalize_squash_commit_message,
};
use serde::{Deserialize, Serialize};

use super::GitHub;
use super::repo::owner_repo_from_pr_url;
use super::timeline::short_sha;
use crate::json::{decode, marshal};
use crate::model::{format_commit_bullet_list, html_escape, pr_description, single_line};
use crate::{ForgeError, MergeCommitText, MergeOutcome};

/// `gh pr merge` argv, by PR number so a local rename cannot break it.
pub(crate) fn merge_args(number: i64, method_flag: &str, msg: &MergeCommitText) -> Vec<String> {
    let mut args = vec![
        "pr".to_owned(),
        "merge".to_owned(),
        number.to_string(),
        method_flag.to_owned(),
    ];
    if method_flag == "--rebase" || msg.subject.is_empty() {
        return args;
    }
    args.extend(["--subject".to_owned(), msg.subject.clone()]);
    if !msg.omit_body {
        args.extend(["--body".to_owned(), msg.body.clone()]);
    }
    args
}

/// `gh api -X PUT repos/<o>/<r>/pulls/<n>/merge` argv.
pub(crate) fn merge_rest_args(owner_repo: &str, number: i64, method: &str) -> Vec<String> {
    vec![
        "api".to_owned(),
        "-X".to_owned(),
        "PUT".to_owned(),
        format!("repos/{owner_repo}/pulls/{number}/merge"),
        "-f".to_owned(),
        format!("merge_method={method}"),
    ]
}

pub(crate) fn merge_cli_flag(method: &MergeMethod) -> &'static str {
    match normalize_merge_method(method.as_str()) {
        MergeMethod::Squash => "--squash",
        MergeMethod::Rebase => "--rebase",
        _ => "--merge",
    }
}

pub(crate) fn graphql_merge_method(method: &MergeMethod) -> &'static str {
    match normalize_merge_method(method.as_str()) {
        MergeMethod::Squash => "SQUASH",
        MergeMethod::Rebase => "REBASE",
        _ => "MERGE",
    }
}

/// How a merge will be executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MergePlan {
    Rest(Vec<String>),
    GraphQl(String),
    Cli(Vec<String>),
}

/// One merge API call when the URL or node id is known (`gh pr merge`
/// re-fetches and waits on mergeability).
pub(crate) fn plan_merge(pr: &Pr, method: &MergeMethod) -> MergePlan {
    let rest_method = normalize_merge_method(method.as_str());
    if let Some(owner_repo) = owner_repo_from_pr_url(&pr.url)
        && pr.number > 0
    {
        return MergePlan::Rest(merge_rest_args(
            &owner_repo,
            pr.number,
            rest_method.as_str(),
        ));
    }
    let id = pr.node_id.trim();
    if !id.is_empty() {
        return MergePlan::GraphQl(id.to_owned());
    }
    MergePlan::Cli(merge_args(
        pr.number,
        merge_cli_flag(method),
        &MergeCommitText::default(),
    ))
}

/// `gh pr merge <url or number>` (enqueue / auto-merge).
pub(crate) fn enqueue_args(pr: &Pr) -> Vec<String> {
    let target = if pr.url.is_empty() {
        pr.number.to_string()
    } else {
        pr.url.clone()
    };
    vec!["pr".to_owned(), "merge".to_owned(), target]
}

pub(crate) fn is_auto_merge_disallowed(err: &str) -> bool {
    err.to_lowercase().contains("auto merge is not allowed")
}

/// GitHub is still computing mergeability (HTTP 405) or the base moved.
pub(crate) fn is_mergeability_pending(err: &str) -> bool {
    let lower = err.to_lowercase();
    lower.contains("not mergeable") || lower.contains("base branch was modified")
}

/// GitHub rejected the gh credentials.
pub(crate) fn is_auth_failure(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    [
        "bad credentials",
        "requires authentication",
        "must authenticate",
        "gh auth login",
        "gh_token",
        "http 401",
    ]
    .iter()
    .any(|m| lower.contains(m))
}

/// User-facing (HTML) note for a failed merge.
pub(crate) fn merge_failure_note(err: &str) -> String {
    let msg = err.trim();
    if is_auth_failure(msg) {
        return format!(
            "Can't merge on GitHub: the gh CLI is not authenticated ({}).",
            html_escape(msg)
        );
    }
    if msg.to_lowercase().contains("the gh cli is not installed") {
        return "Can't merge on GitHub: the gh CLI is not installed.".to_owned();
    }
    format!("GitHub merge failed: {}", html_escape(msg))
}

/// Up to three attempts while mergeability is pending, `delay` apart.
pub(crate) async fn retry_rest_merge<F, Fut>(
    delay: Duration,
    mut attempt: F,
) -> Result<(), ForgeError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), ForgeError>>,
{
    let mut last = Ok(());
    for i in 0..3 {
        if i > 0 && !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        last = attempt().await;
        match &last {
            Err(e) if is_mergeability_pending(&e.message) => {}
            _ => return last,
        }
    }
    last
}

#[derive(Serialize)]
struct GraphQlPayload<'a, V: Serialize> {
    query: &'a str,
    variables: V,
}

const ENQUEUE_MUTATION: &str =
    "mutation($id:ID!){enqueuePullRequest(input:{pullRequestId:$id}){mergeQueueEntry{id}}}";
const MERGE_MUTATION: &str = "mutation($id:ID!,$method:PullRequestMergeMethod!){mergePullRequest(input:{pullRequestId:$id,mergeMethod:$method}){pullRequest{merged}}}";

impl GitHub {
    /// The query on stdin; an `"errors"` key in the answer is a failure.
    pub(crate) async fn graphql_mutation<V: Serialize>(
        &self,
        repo: &Path,
        name: &str,
        query: &str,
        variables: V,
    ) -> Result<(), ForgeError> {
        let raw = marshal(&GraphQlPayload { query, variables });
        let out = self
            .gh
            .run_stdin(repo, &raw, &["api", "graphql", "--input", "-"])
            .await
            .map_err(|e| ForgeError::wrap(format!("gh api graphql {name}"), e))?;
        if out.contains("\"errors\"") {
            return Err(ForgeError::failed(format!(
                "gh api graphql {name}: {}",
                out.trim()
            )));
        }
        Ok(())
    }

    pub(crate) async fn merge_pr(
        &self,
        repo: &Path,
        pr: &Pr,
        method: &MergeMethod,
    ) -> Result<MergeOutcome, ForgeError> {
        if !self.gh.installed() {
            return Err(ForgeError::unavailable("the gh CLI is not installed"));
        }
        if pr.merge_queue_enabled {
            return self.enqueue(repo, pr).await;
        }
        let res = match plan_merge(pr, method) {
            MergePlan::Rest(args) => {
                retry_rest_merge(self.merge_retry_delay, || async {
                    self.gh
                        .run(repo, &args)
                        .await
                        .map(drop)
                        .map_err(ForgeError::from)
                })
                .await
            }
            MergePlan::GraphQl(id) => {
                let vars = std::collections::BTreeMap::from([
                    ("id", id.as_str()),
                    ("method", graphql_merge_method(method)),
                ]);
                self.graphql_mutation(repo, "mergePullRequest", MERGE_MUTATION, vars)
                    .await
            }
            MergePlan::Cli(args) => self
                .gh
                .run(repo, &args)
                .await
                .map(drop)
                .map_err(ForgeError::from),
        };
        match res {
            // The queue was enabled after the last snapshot; the merge APIs refuse it.
            Err(e) if e.message.to_lowercase().contains("merge queue") => {
                self.enqueue(repo, pr).await
            }
            Err(e) => Err(e),
            Ok(()) => Ok(MergeOutcome::Merged),
        }
    }

    /// `gh pr merge` enables auto-merge into the queue; repos with auto-merge
    /// disabled get `enqueuePullRequest`.
    async fn enqueue(&self, repo: &Path, pr: &Pr) -> Result<MergeOutcome, ForgeError> {
        let mut res = self
            .gh
            .run(repo, &enqueue_args(pr))
            .await
            .map(drop)
            .map_err(ForgeError::from);
        let id = pr.node_id.trim();
        if let Err(e) = &res
            && !id.is_empty()
            && is_auto_merge_disallowed(&e.message)
        {
            res = self
                .graphql_mutation(
                    repo,
                    "enqueuePullRequest",
                    ENQUEUE_MUTATION,
                    std::collections::BTreeMap::from([("id", id)]),
                )
                .await;
        }
        res.map(|()| MergeOutcome::Queued)
    }

    /// Live title, body and commits.
    pub async fn merge_fields(&self, repo: &Path, number: i64) -> Result<MergeFields, ForgeError> {
        if number <= 0 {
            return Err(ForgeError::invalid(format!(
                "invalid GitHub PR number {number}"
            )));
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
                    "title,body,commits".to_owned(),
                ],
            )
            .await?;
        parse_merge_fields(&out).map_err(ForgeError::failed)
    }
}

/// A PR's live title, description and commits (newest first).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeFields {
    pub title: String,
    pub body: String,
    pub commits: Vec<Commit>,
}

pub fn parse_merge_fields(raw: &str) -> Result<MergeFields, String> {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct View {
        title: String,
        body: String,
        commits: Vec<super::thread::ViewCommit>,
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("empty gh pr view json".into());
    }
    let v: View = decode(raw).map_err(|e| format!("parse gh pr view json: {e}"))?;
    Ok(MergeFields {
        title: v.title.trim().to_owned(),
        body: v.body.trim().to_owned(),
        commits: commits_newest_first(&v.commits),
    })
}

/// `gh pr view --json commits` (oldest first) as Recur commits (newest first).
fn commits_newest_first(cs: &[super::thread::ViewCommit]) -> Vec<Commit> {
    let mut out: Vec<Commit> = cs
        .iter()
        .filter_map(|c| {
            let msg = c.message_headline.trim();
            if msg.is_empty() {
                return None;
            }
            let oid = c.oid.trim();
            Some(Commit {
                message: msg.to_owned(),
                body: c.message_body.trim().to_owned(),
                sha: oid.to_owned(),
                hash: short_sha(oid),
                ..Default::default()
            })
        })
        .collect();
    out.reverse();
    out
}

/// The live title, commits and description replace the stored ones.
pub fn overlay_merge_fields(mut pr: Pr, fields: &MergeFields) -> Pr {
    pr.title = fields.title.trim().to_owned();
    pr.commits = Some(fields.commits.clone());
    let body = fields.body.trim().to_owned();
    let comments = pr.comments.get_or_insert_with(Vec::new);
    match comments.iter_mut().find(|c| c.kind == "description") {
        Some(c) => c.body_md = body,
        None => comments.push(PrComment {
            kind: "description".into(),
            body_md: body,
            ..Default::default()
        }),
    }
    pr
}

/// On a fetch error: drop cached title, commits and comments so the merge omits
/// `--subject`/`--body` rather than overwriting GitHub's message with a stale
/// copy.
pub fn without_merge_fields(mut pr: Pr) -> Pr {
    pr.title.clear();
    pr.commits = None;
    pr.comments = None;
    pr
}

/// A GitHub PR's merge message: `<title> (#n)`; an empty title
/// lets GitHub choose; rebase carries no message.
pub(crate) fn merge_message(
    pr: &Pr,
    method: &MergeMethod,
    squash: &SquashCommitMessage,
    merge: &MergeCommitMessage,
) -> MergeCommitText {
    let title = single_line(&pr.title);
    if title.is_empty() {
        return MergeCommitText::default();
    }
    let subject = if pr.number > 0 {
        format!("{title} (#{})", pr.number)
    } else {
        title
    };
    let desc = pr_description(pr);
    match method.as_str() {
        "squash" => {
            let style = normalize_squash_commit_message(squash.as_str());
            let commits = pr.commits.as_deref().unwrap_or_default();
            let mut parts = Vec::new();
            let mut commit_list = String::new();
            match style {
                SquashCommitMessage::TitleAndCommits => {
                    commit_list = format_commit_bullet_list(commits);
                    if !commit_list.is_empty() {
                        parts.push(commit_list.clone());
                    }
                }
                SquashCommitMessage::TitleAndDescription if !desc.is_empty() => parts.push(desc),
                _ => {}
            }
            MergeCommitText {
                subject,
                body: parts.join("\n\n"),
                omit_body: style == SquashCommitMessage::TitleAndCommits && commit_list.is_empty(),
            }
        }
        "rebase" => MergeCommitText::default(),
        _ => {
            let style = normalize_merge_commit_message(merge.as_str());
            MergeCommitText {
                subject,
                body: if style == MergeCommitMessage::TitleAndDescription {
                    desc
                } else {
                    String::new()
                },
                omit_body: false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_args_use_number() {
        assert_eq!(
            merge_args(482, "--squash", &MergeCommitText::default()),
            ["pr", "merge", "482", "--squash"]
        );
        assert_eq!(
            merge_args(
                482,
                "--squash",
                &MergeCommitText {
                    subject: "Fix foo (#482)".into(),
                    body: "* commit a".into(),
                    omit_body: false
                }
            ),
            [
                "pr",
                "merge",
                "482",
                "--squash",
                "--subject",
                "Fix foo (#482)",
                "--body",
                "* commit a"
            ]
        );
        assert_eq!(
            merge_args(
                7,
                "--rebase",
                &MergeCommitText {
                    subject: "ignored".into(),
                    body: "ignored".into(),
                    omit_body: false
                }
            ),
            ["pr", "merge", "7", "--rebase"]
        );
        assert_eq!(
            merge_args(
                11,
                "--squash",
                &MergeCommitText {
                    subject: "Only title (#11)".into(),
                    body: String::new(),
                    omit_body: true
                }
            ),
            [
                "pr",
                "merge",
                "11",
                "--squash",
                "--subject",
                "Only title (#11)"
            ]
        );
        assert_eq!(
            merge_rest_args("acme/app", 12, "squash"),
            [
                "api",
                "-X",
                "PUT",
                "repos/acme/app/pulls/12/merge",
                "-f",
                "merge_method=squash"
            ]
        );
    }

    #[test]
    fn plan_prefers_rest_then_graphql_then_cli() {
        let pr = Pr {
            number: 12,
            url: "https://github.com/acme/app/pull/12".into(),
            node_id: "PR_1".into(),
            ..Default::default()
        };
        assert_eq!(
            plan_merge(&pr, &MergeMethod::Squash),
            MergePlan::Rest(merge_rest_args("acme/app", 12, "squash"))
        );
        let pr = Pr {
            number: 12,
            node_id: "PR_kwDO".into(),
            ..Default::default()
        };
        assert_eq!(
            plan_merge(&pr, &MergeMethod::Merge),
            MergePlan::GraphQl("PR_kwDO".into())
        );
        assert_eq!(graphql_merge_method(&MergeMethod::Squash), "SQUASH");
        assert_eq!(graphql_merge_method(&MergeMethod::Rebase), "REBASE");
        assert_eq!(graphql_merge_method(&MergeMethod::Merge), "MERGE");
        let pr = Pr {
            number: 482,
            ..Default::default()
        };
        assert_eq!(
            plan_merge(&pr, &MergeMethod::Rebase),
            MergePlan::Cli(vec![
                "pr".into(),
                "merge".into(),
                "482".into(),
                "--rebase".into()
            ])
        );
    }

    #[test]
    fn enqueue_targets_url_then_number() {
        let pr = Pr {
            number: 20,
            url: "https://github.com/acme/app/pull/20".into(),
            ..Default::default()
        };
        assert_eq!(
            enqueue_args(&pr),
            ["pr", "merge", "https://github.com/acme/app/pull/20"]
        );
        let pr = Pr {
            number: 20,
            ..Default::default()
        };
        assert_eq!(enqueue_args(&pr), ["pr", "merge", "20"]);
        assert!(is_auto_merge_disallowed(
            "gh pr merge https://github.com/celestiaorg/recur/pull/37: GraphQL: Auto merge is not allowed for this repository (enablePullRequestAutoMerge)"
        ));
        assert!(!is_auto_merge_disallowed(
            "GraphQL: Required status check \"test\" is expected"
        ));
    }

    #[test]
    fn failure_notes() {
        for (err, want, not_want) in [
            (
                "gh api -X PUT repos/acme/app/pulls/88/merge: gh: Bad credentials (HTTP 401)",
                "not authenticated",
                "",
            ),
            (
                "gh api graphql mergePullRequest: HTTP 403: Resource not accessible by personal access token",
                "GitHub merge failed",
                "not authenticated",
            ),
            (
                "gh: API rate limit exceeded for user ID 1234. (HTTP 403)",
                "rate limit exceeded",
                "not authenticated",
            ),
            (
                "Resource protected by organization SAML enforcement. You must grant your OAuth token access to this organization. (HTTP 403)",
                "SAML enforcement",
                "not authenticated",
            ),
            ("the gh CLI is not installed", "not installed", ""),
            (
                "To get started with GitHub CLI, please run:  gh auth login\nAlternatively, populate the GH_TOKEN environment variable",
                "not authenticated",
                "",
            ),
            (
                "Pull Request is not mergeable",
                "GitHub merge failed: Pull Request is not mergeable",
                "",
            ),
        ] {
            let got = merge_failure_note(err);
            assert!(got.contains(want), "{err:?} → {got:?}");
            assert!(
                not_want.is_empty() || !got.contains(not_want),
                "{err:?} → {got:?}"
            );
        }
        assert!(merge_failure_note("gh: Bad credentials (HTTP 401)").contains("Bad credentials"));
    }

    #[tokio::test]
    async fn retry_rest_merge_retries_mergeability_only() {
        let n = std::cell::Cell::new(0);
        let res = retry_rest_merge(Duration::ZERO, || {
            n.set(n.get() + 1);
            let k = n.get();
            async move {
                if k < 3 {
                    Err(ForgeError::failed(
                        "gh api PUT: gh: Pull Request is not mergeable (HTTP 405)",
                    ))
                } else {
                    Ok(())
                }
            }
        })
        .await;
        assert!(res.is_ok() && n.get() == 3);

        let n = std::cell::Cell::new(0);
        let res = retry_rest_merge(Duration::ZERO, || {
            n.set(n.get() + 1);
            async {
                Err(ForgeError::failed(
                    "gh api PUT: required status checks have not passed",
                ))
            }
        })
        .await;
        assert!(res.is_err() && n.get() == 1);
    }

    fn desc(body: &str) -> PrComment {
        PrComment {
            kind: "description".into(),
            body_md: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn merge_fields_parse_and_overlay() {
        let got = parse_merge_fields(
            r#"{
                "title": "Live PR title",
                "body": "The real description.",
                "commits": [
                    {"oid": "aaa1111", "messageHeadline": "oldest commit", "messageBody": "detail"},
                    {"oid": "bbb2222", "messageHeadline": "newest commit"}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(
            (got.title.as_str(), got.body.as_str()),
            ("Live PR title", "The real description.")
        );
        assert_eq!(got.commits.len(), 2);
        assert_eq!(got.commits[0].message, "newest commit");
        assert_eq!(got.commits[1].message, "oldest commit");

        let overlay = overlay_merge_fields(
            Pr {
                title: "stale".into(),
                commits: Some(vec![Commit {
                    message: "stale commit".into(),
                    ..Default::default()
                }]),
                comments: Some(vec![desc("stale body")]),
                ..Default::default()
            },
            &got,
        );
        assert_eq!(overlay.title, "Live PR title");
        assert_eq!(pr_description(&overlay), "The real description.");
        assert_eq!(
            overlay.commits.as_ref().unwrap()[0].message,
            "newest commit"
        );
        assert!(parse_merge_fields("").is_err());
    }

    #[test]
    fn fetch_error_omits_merge_flags() {
        let orig = Pr {
            number: 12,
            source: recur_domain::PrSource::GitHub,
            title: "stale title".into(),
            commits: Some(vec![Commit {
                message: "stale commit".into(),
                ..Default::default()
            }]),
            comments: Some(vec![desc("stale body")]),
            ..Default::default()
        };
        let got = without_merge_fields(orig.clone());
        let msg = merge_message(
            &got,
            &MergeMethod::Squash,
            &SquashCommitMessage::TitleAndCommits,
            &MergeCommitMessage::Title,
        );
        assert_eq!(
            merge_args(orig.number, "--squash", &msg),
            ["pr", "merge", "12", "--squash"]
        );
    }

    #[test]
    fn live_title_drives_subject() {
        let pr = overlay_merge_fields(
            Pr {
                number: 12,
                title: "stale".into(),
                comments: Some(vec![desc("stale body")]),
                ..Default::default()
            },
            &MergeFields {
                title: "Live title".into(),
                body: "Live body".into(),
                commits: Vec::new(),
            },
        );
        let msg = merge_message(
            &pr,
            &MergeMethod::Merge,
            &SquashCommitMessage::TitleAndCommits,
            &MergeCommitMessage::Title,
        );
        assert_eq!(msg.subject, "Live title (#12)");
        assert_eq!(pr_description(&pr), "Live body");
    }

    #[test]
    fn squash_message_styles() {
        let pr = Pr {
            number: 7,
            title: "Add OAuth\nmore".into(),
            commits: Some(vec![
                Commit {
                    message: "second".into(),
                    ..Default::default()
                },
                Commit {
                    message: "first".into(),
                    ..Default::default()
                },
            ]),
            comments: Some(vec![desc(" The why ")]),
            ..Default::default()
        };
        let m = |method, squash, merge| merge_message(&pr, &method, &squash, &merge);
        assert_eq!(
            m(
                MergeMethod::Squash,
                SquashCommitMessage::TitleAndCommits,
                MergeCommitMessage::Title
            ),
            MergeCommitText {
                subject: "Add OAuth (#7)".into(),
                body: "* first\n* second".into(),
                omit_body: false
            }
        );
        assert_eq!(
            m(
                MergeMethod::Squash,
                SquashCommitMessage::TitleAndDescription,
                MergeCommitMessage::Title
            )
            .body,
            "The why"
        );
        assert_eq!(
            m(
                MergeMethod::Squash,
                SquashCommitMessage::Title,
                MergeCommitMessage::Title
            )
            .body,
            ""
        );
        assert_eq!(
            m(
                MergeMethod::Merge,
                SquashCommitMessage::Title,
                MergeCommitMessage::TitleAndDescription
            ),
            MergeCommitText {
                subject: "Add OAuth (#7)".into(),
                body: "The why".into(),
                omit_body: false
            }
        );
        assert_eq!(
            m(
                MergeMethod::Rebase,
                SquashCommitMessage::Title,
                MergeCommitMessage::Title
            ),
            MergeCommitText::default()
        );
        let bare = Pr {
            number: 7,
            title: "T".into(),
            ..Default::default()
        };
        assert!(
            merge_message(
                &bare,
                &MergeMethod::Squash,
                &SquashCommitMessage::TitleAndCommits,
                &MergeCommitMessage::Title
            )
            .omit_body
        );
    }
}
