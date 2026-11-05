//! Writes to a GitHub PR: comments, line notes, reviews, edits, deletes,
//! replies, thread resolution, draft state, plus viewer and mention lookups.

use std::path::Path;

use chrono::Utc;
use recur_domain::{MergeMethod, PendingLineComment, PrComment, PrParticipant};
use serde::{Deserialize, Serialize};

use super::GitHub;
use super::repo::{owner_repo_from_pr_url, split_owner_repo};
use super::thread::{parse_mentionable_users, parse_posted_inline_comment};
use crate::json::{decode, marshal};
use crate::{ForgeError, Viewer};

/// `LEFT` or `RIGHT` for a pending line note.
pub(crate) fn pending_side(side: &str) -> &'static str {
    if side.eq_ignore_ascii_case("LEFT") {
        "LEFT"
    } else {
        "RIGHT"
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RepoPermissions {
    pub admin: bool,
    pub maintain: bool,
    pub push: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct RepoSettings {
    pub permissions: RepoPermissions,
    pub allow_merge_commit: Option<bool>,
    pub allow_squash_merge: Option<bool>,
    pub allow_rebase_merge: Option<bool>,
}

pub(crate) fn parse_repo_settings(raw: &str) -> Option<RepoSettings> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    decode(raw).ok()
}

/// Enabled merge methods; empty when the repo did not report any flag.
pub(crate) fn allowed_merge_methods_from_repo(r: &RepoSettings) -> Vec<String> {
    if r.allow_merge_commit.is_none()
        && r.allow_squash_merge.is_none()
        && r.allow_rebase_merge.is_none()
    {
        return Vec::new();
    }
    [
        (r.allow_merge_commit, MergeMethod::Merge),
        (r.allow_squash_merge, MergeMethod::Squash),
        (r.allow_rebase_merge, MergeMethod::Rebase),
    ]
    .into_iter()
    .filter(|(on, _)| on.unwrap_or(true))
    .map(|(_, m)| m.as_str().to_owned())
    .collect()
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct HeadMeta {
    url: String,
    head_ref_oid: String,
}

#[derive(Serialize)]
struct ReviewComment {
    path: String,
    line: i64,
    side: String,
    body: String,
    #[serde(skip_serializing_if = "is_zero")]
    start_line: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    start_side: String,
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

#[derive(Serialize)]
struct ReviewPayload {
    commit_id: String,
    body: String,
    event: String,
    comments: Option<Vec<ReviewComment>>,
}

#[derive(Serialize)]
struct Mutation<'a, V: Serialize> {
    query: &'a str,
    variables: V,
}

/// The `gh api graphql` failure check for stdin mutations.
fn graphql_result(name: &str, res: Result<String, crate::cli::CmdError>) -> Result<(), ForgeError> {
    let out = res.map_err(|e| ForgeError::wrap(format!("gh api graphql {name}"), e))?;
    if out.contains("\"errors\"") {
        return Err(ForgeError::failed(format!(
            "gh api graphql {name}: {}",
            out.trim()
        )));
    }
    Ok(())
}

impl GitHub {
    async fn head_meta(&self, repo: &Path, number: i64) -> Result<HeadMeta, ForgeError> {
        let out = self
            .gh
            .run(
                repo,
                &[
                    "pr".to_owned(),
                    "view".to_owned(),
                    number.to_string(),
                    "--json".to_owned(),
                    "url,headRefOid".to_owned(),
                ],
            )
            .await?;
        decode(out.trim()).map_err(|e| ForgeError::failed(format!("parse gh pr view: {e}")))
    }

    /// A line note when `file` and `line` are set, falling back to a
    /// conversation comment prefixed with the location.
    pub(crate) async fn post(
        &self,
        repo: &Path,
        number: i64,
        file: &str,
        line: i64,
        body: &str,
    ) -> Result<(), ForgeError> {
        let mut body = body.trim().to_owned();
        if body.is_empty() {
            return Err(ForgeError::invalid("empty comment"));
        }
        if !file.trim().is_empty() && line > 0 {
            if self
                .post_inline(repo, number, file, line, &body)
                .await
                .is_ok()
            {
                return Ok(());
            }
            body = format!("{file}:{line}\n\n{body}");
        }
        self.gh
            .run(
                repo,
                &[
                    "pr".to_owned(),
                    "comment".to_owned(),
                    number.to_string(),
                    "--body".to_owned(),
                    body,
                ],
            )
            .await
            .map(drop)
            .map_err(|e| ForgeError::wrap(format!("gh pr comment {number}"), e))
    }

    async fn post_inline(
        &self,
        repo: &Path,
        number: i64,
        file: &str,
        line: i64,
        body: &str,
    ) -> Result<(), ForgeError> {
        let meta = self.head_meta(repo, number).await?;
        let owner_repo = owner_repo_from_pr_url(&meta.url)
            .filter(|_| !meta.head_ref_oid.is_empty())
            .ok_or_else(|| ForgeError::failed("missing repo or head oid for inline comment"))?;
        self.gh
            .run(
                repo,
                &[
                    "api".to_owned(),
                    format!("repos/{owner_repo}/pulls/{number}/comments"),
                    "-f".to_owned(),
                    format!("body={body}"),
                    "-f".to_owned(),
                    format!("commit_id={}", meta.head_ref_oid),
                    "-f".to_owned(),
                    format!("path={file}"),
                    "-F".to_owned(),
                    format!("line={line}"),
                    "-f".to_owned(),
                    "side=RIGHT".to_owned(),
                ],
            )
            .await?;
        Ok(())
    }

    /// `kind` is `approve`, `request` or a comment.
    pub(crate) async fn review(
        &self,
        repo: &Path,
        number: i64,
        kind: &str,
        body: &str,
        comments: &[PendingLineComment],
    ) -> Result<(), ForgeError> {
        let mut body = body.trim().to_owned();
        let (flag, event, default) = match kind {
            "approve" => ("--approve", "APPROVE", "Approved"),
            "request" => ("--request-changes", "REQUEST_CHANGES", "Requested changes"),
            _ => ("--comment", "COMMENT", "Commented"),
        };
        if body.is_empty() {
            body = default.to_owned();
        }
        if !comments.is_empty() {
            return self
                .review_with_comments(repo, number, event, &body, comments)
                .await;
        }
        self.gh
            .run(
                repo,
                &[
                    "pr".to_owned(),
                    "review".to_owned(),
                    number.to_string(),
                    flag.to_owned(),
                    "--body".to_owned(),
                    body,
                ],
            )
            .await
            .map(drop)
            .map_err(|e| ForgeError::wrap(format!("gh pr review {number}"), e))
    }

    async fn review_with_comments(
        &self,
        repo: &Path,
        number: i64,
        event: &str,
        body: &str,
        comments: &[PendingLineComment],
    ) -> Result<(), ForgeError> {
        let meta = self.head_meta(repo, number).await?;
        let owner_repo = owner_repo_from_pr_url(&meta.url)
            .filter(|_| !meta.head_ref_oid.is_empty())
            .ok_or_else(|| ForgeError::failed("missing repo or head oid for review"))?;
        let mut rows = Vec::new();
        for pc in comments {
            let side = pending_side(&pc.side);
            if pc.line <= 0 || pc.file.trim().is_empty() || pc.body.trim().is_empty() {
                continue;
            }
            let mut rc = ReviewComment {
                path: pc.file.clone(),
                line: pc.line,
                side: side.to_owned(),
                body: pc.body.clone(),
                start_line: 0,
                start_side: String::new(),
            };
            if pc.start_line > 0 && pc.start_line < pc.line {
                rc.start_line = pc.start_line;
                rc.start_side = if pc.start_side.is_empty() {
                    side.to_owned()
                } else {
                    pending_side(&pc.start_side).to_owned()
                };
            }
            rows.push(rc);
        }
        let payload = ReviewPayload {
            commit_id: meta.head_ref_oid,
            body: body.to_owned(),
            event: event.to_owned(),
            comments: (!rows.is_empty()).then_some(rows),
        };
        self.gh
            .run_stdin(
                repo,
                &marshal(&payload),
                &[
                    "api",
                    &format!("repos/{owner_repo}/pulls/{number}/reviews"),
                    "--method",
                    "POST",
                    "--input",
                    "-",
                ],
            )
            .await
            .map(drop)
            .map_err(|e| ForgeError::wrap(format!("gh api pull review {number}"), e))
    }

    /// Login, write access and merge methods.
    pub(crate) async fn fetch_viewer(&self, repo: &Path, owner_repo: &str) -> Viewer {
        let repo_path = format!("repos/{owner_repo}");
        let settings_args = [
            "api",
            &repo_path,
            "-q",
            "{permissions: .permissions, allow_merge_commit: .allow_merge_commit, allow_squash_merge: .allow_squash_merge, allow_rebase_merge: .allow_rebase_merge}",
        ];
        let (login, settings) = tokio::join!(
            self.gh.run(repo, &["api", "user", "-q", ".login"]),
            self.gh.run(repo, &settings_args)
        );
        let Ok(login) = login else {
            return Viewer::default();
        };
        let mut v = Viewer {
            login: login.trim().to_owned(),
            ..Default::default()
        };
        let Some(s) = settings.ok().and_then(|out| parse_repo_settings(&out)) else {
            return v;
        };
        v.can_write = s.permissions.admin || s.permissions.maintain || s.permissions.push;
        v.allowed_merge_methods = allowed_merge_methods_from_repo(&s);
        v
    }

    /// `gh pr ready` (or `--undo` to convert to draft).
    pub(crate) async fn draft(
        &self,
        repo: &Path,
        number: i64,
        draft: bool,
    ) -> Result<(), ForgeError> {
        if number <= 0 {
            return Err(ForgeError::invalid(format!(
                "invalid GitHub PR number {number}"
            )));
        }
        let mut args = vec!["pr".to_owned(), "ready".to_owned(), number.to_string()];
        if draft {
            args.push("--undo".to_owned());
        }
        self.gh.run(repo, &args).await.map(drop).map_err(|e| {
            let what = if draft {
                "gh pr ready --undo"
            } else {
                "gh pr ready"
            };
            ForgeError::wrap(format!("{what} {number}"), e)
        })
    }

    /// `gh pr close`.
    pub(crate) async fn close(&self, repo: &Path, number: i64) -> Result<(), ForgeError> {
        if number <= 0 {
            return Err(ForgeError::invalid(format!(
                "invalid GitHub PR number {number}"
            )));
        }
        self.gh
            .run(repo, &["pr".into(), "close".into(), number.to_string()])
            .await
            .map(drop)
            .map_err(|e| ForgeError::wrap(format!("gh pr close {number}"), e))
    }

    /// Title, and the body unless `None`.
    pub(crate) async fn edit(
        &self,
        repo: &Path,
        number: i64,
        title: &str,
        body: Option<&str>,
    ) -> Result<(), ForgeError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(ForgeError::invalid("empty title"));
        }
        let mut args = vec![
            "pr".to_owned(),
            "edit".to_owned(),
            number.to_string(),
            "--title".to_owned(),
            title.to_owned(),
        ];
        if let Some(body) = body {
            args.extend(["--body".to_owned(), body.to_owned()]);
        }
        self.gh
            .run(repo, &args)
            .await
            .map(drop)
            .map_err(|e| ForgeError::wrap(format!("gh pr edit {number}"), e))
    }

    /// Description, issue comment, review body or line note, chosen by the
    /// Recur row id.
    pub(crate) async fn edit_comment_body(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(ForgeError::invalid("empty comment"));
        }
        let id = comment_id.trim();
        if id == "gh-description" {
            return self
                .gh
                .run(
                    repo,
                    &[
                        "pr".to_owned(),
                        "edit".to_owned(),
                        number.to_string(),
                        "--body".to_owned(),
                        body.to_owned(),
                    ],
                )
                .await
                .map(drop)
                .map_err(|e| ForgeError::wrap(format!("gh pr edit {number} body"), e));
        }
        if let Some(node) = id.strip_prefix("gh-comment-") {
            return self
                .edit_graphql_comment(repo, "updateIssueComment", "id", node, body)
                .await;
        }
        if let Some(node) = id.strip_prefix("gh-review-") {
            return self
                .edit_graphql_comment(
                    repo,
                    "updatePullRequestReview",
                    "pullRequestReviewId",
                    node,
                    body,
                )
                .await;
        }
        if let Some(rest) = id.strip_prefix("gh-inline-") {
            return self.edit_inline(repo, number, rest, body).await;
        }
        Err(ForgeError::invalid(format!("unsupported comment id {id}")))
    }

    async fn edit_graphql_comment(
        &self,
        repo: &Path,
        mutation: &str,
        id_field: &str,
        node_id: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        let node_id = node_id.trim();
        if node_id.is_empty() {
            return Err(ForgeError::invalid("empty github comment id"));
        }
        let query = format!(
            "mutation($id:ID!,$body:String!){{{mutation}(input:{{{id_field}:$id,body:$body}}){{clientMutationId}}}}"
        );
        let vars = std::collections::BTreeMap::from([("body", body), ("id", node_id)]);
        let raw = marshal(&Mutation {
            query: &query,
            variables: vars,
        });
        graphql_result(
            mutation,
            self.gh
                .run_stdin(repo, &raw, &["api", "graphql", "--input", "-"])
                .await,
        )
    }

    async fn edit_inline(
        &self,
        repo: &Path,
        number: i64,
        rest_id: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        if rest_id.parse::<i64>().is_err() {
            return Err(ForgeError::invalid(format!(
                "invalid inline comment id {rest_id:?}"
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
                    "url".to_owned(),
                ],
            )
            .await?;
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Meta {
            url: String,
        }
        let meta: Meta =
            decode(out.trim()).map_err(|e| ForgeError::failed(format!("parse gh pr view: {e}")))?;
        let owner_repo = owner_repo_from_pr_url(&meta.url)
            .ok_or_else(|| ForgeError::failed("missing repo for inline comment edit"))?;
        self.gh
            .run(
                repo,
                &[
                    "api".to_owned(),
                    format!("repos/{owner_repo}/pulls/comments/{rest_id}"),
                    "-X".to_owned(),
                    "PATCH".to_owned(),
                    "-f".to_owned(),
                    format!("body={body}"),
                ],
            )
            .await
            .map(drop)
            .map_err(|e| ForgeError::wrap(format!("gh api pull comment {rest_id}"), e))
    }

    /// Issue comments and line notes.
    pub(crate) async fn delete(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
    ) -> Result<(), ForgeError> {
        let id = comment_id.trim();
        if let Some(node) = id.strip_prefix("gh-comment-") {
            let node = node.trim();
            if node.is_empty() {
                return Err(ForgeError::invalid("empty github comment id"));
            }
            let query = "mutation($id:ID!){deleteIssueComment(input:{id:$id}){clientMutationId}}";
            let vars = std::collections::BTreeMap::from([("id", node)]);
            let raw = marshal(&Mutation {
                query,
                variables: vars,
            });
            return graphql_result(
                "deleteIssueComment",
                self.gh
                    .run_stdin(repo, &raw, &["api", "graphql", "--input", "-"])
                    .await,
            );
        }
        if let Some(rest) = id.strip_prefix("gh-inline-") {
            if rest.parse::<i64>().is_err() {
                return Err(ForgeError::invalid(format!(
                    "invalid inline comment id {id:?}"
                )));
            }
            let owner_repo = self.owner_repo_for_pr(repo, number).await?;
            return self
                .gh
                .run(
                    repo,
                    &[
                        "api",
                        &format!("repos/{owner_repo}/pulls/comments/{rest}"),
                        "-X",
                        "DELETE",
                    ],
                )
                .await
                .map(drop)
                .map_err(|e| ForgeError::wrap(format!("gh api delete pull comment {rest}"), e));
        }
        Err(ForgeError::invalid(format!("unsupported comment id {id}")))
    }

    /// The reply row GitHub created, or `None` when its answer did not parse.
    pub(crate) async fn reply_to(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
        pr_url: &str,
    ) -> Result<Option<PrComment>, ForgeError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(ForgeError::invalid("empty comment"));
        }
        let id = comment_id.trim();
        let rest = id.strip_prefix("gh-inline-").unwrap_or(id);
        if rest.parse::<i64>().is_err() {
            return Err(ForgeError::invalid(format!(
                "invalid inline comment id {comment_id:?}"
            )));
        }
        let owner_repo = self.owner_repo_for_pr(repo, number).await?;
        let out = self
            .gh
            .run(
                repo,
                &[
                    "api".to_owned(),
                    format!("repos/{owner_repo}/pulls/{number}/comments"),
                    "-f".to_owned(),
                    format!("body={body}"),
                    "-F".to_owned(),
                    format!("in_reply_to={rest}"),
                ],
            )
            .await
            .map_err(|e| ForgeError::wrap(format!("gh api reply pull comment {rest}"), e))?;
        Ok(parse_posted_inline_comment(&out, Utc::now(), pr_url))
    }

    pub(crate) async fn resolve_thread(
        &self,
        repo: &Path,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), ForgeError> {
        let thread_id = thread_id.trim();
        if thread_id.is_empty() {
            return Err(ForgeError::invalid("empty review thread id"));
        }
        let mutation = if resolved {
            "resolveReviewThread"
        } else {
            "unresolveReviewThread"
        };
        let query = format!(
            "mutation($id:ID!){{{mutation}(input:{{threadId:$id}}){{thread{{isResolved}}}}}}"
        );
        let vars = std::collections::BTreeMap::from([("id", thread_id)]);
        let raw = marshal(&Mutation {
            query: &query,
            variables: vars,
        });
        graphql_result(
            mutation,
            self.gh
                .run_stdin(repo, &raw, &["api", "graphql", "--input", "-"])
                .await,
        )
    }

    /// People who can be @mentioned in the PR's repo. String variables go as
    /// `-f` so a query like `123` stays a string.
    pub(crate) async fn search_mentionable(
        &self,
        repo: &Path,
        pr_url: &str,
        query: &str,
    ) -> Vec<PrParticipant> {
        let Some((owner, name)) = owner_repo_from_pr_url(pr_url).and_then(|r| split_owner_repo(&r))
        else {
            return Vec::new();
        };
        const Q: &str = "query($o:String!,$r:String!,$q:String){repository(owner:$o,name:$r){mentionableUsers(first:10,query:$q){nodes{login name avatarUrl}}}}";
        let res = self
            .gh
            .run(
                repo,
                &[
                    "api".to_owned(),
                    "graphql".to_owned(),
                    "-f".to_owned(),
                    format!("query={Q}"),
                    "-f".to_owned(),
                    format!("o={owner}"),
                    "-f".to_owned(),
                    format!("r={name}"),
                    "-f".to_owned(),
                    format!("q={}", query.trim()),
                ],
            )
            .await;
        res.map(|out| parse_mentionable_users(&out))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_merge_methods() {
        let s = parse_repo_settings(
            r#"{"permissions": {"push": true}, "allow_merge_commit": false, "allow_squash_merge": true, "allow_rebase_merge": false}"#,
        )
        .unwrap();
        assert_eq!(allowed_merge_methods_from_repo(&s), ["squash"]);
        let unknown = parse_repo_settings(r#"{"permissions": {"push": true}}"#).unwrap();
        assert!(allowed_merge_methods_from_repo(&unknown).is_empty());
        let partial = parse_repo_settings(r#"{"allow_squash_merge": false}"#).unwrap();
        assert_eq!(
            allowed_merge_methods_from_repo(&partial),
            ["merge", "rebase"]
        );
    }

    #[test]
    fn review_payload_matches_go_json() {
        let payload = ReviewPayload {
            commit_id: "abc".into(),
            body: "Approved".into(),
            event: "APPROVE".into(),
            comments: Some(vec![ReviewComment {
                path: "a.go".into(),
                line: 9,
                side: "RIGHT".into(),
                body: "x<y".into(),
                start_line: 7,
                start_side: "RIGHT".into(),
            }]),
        };
        assert_eq!(
            marshal(&payload),
            r#"{"commit_id":"abc","body":"Approved","event":"APPROVE","comments":[{"path":"a.go","line":9,"side":"RIGHT","body":"x<y","start_line":7,"start_side":"RIGHT"}]}"#
        );
        let none = ReviewPayload {
            commit_id: String::new(),
            body: String::new(),
            event: String::new(),
            comments: None,
        };
        assert!(marshal(&none).ends_with(r#""comments":null}"#));
    }
}
