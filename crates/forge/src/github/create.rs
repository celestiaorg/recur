//! Opening GitHub PRs (`git push` + `gh pr create`) and pushing PR heads.

use std::path::Path;

use recur_domain::Pr;
use serde::Deserialize;

use super::GitHub;
use super::repo::{fork_remote_url, pr_number_from_url};
use crate::cli::{CmdError, git};
use crate::json::decode;
use crate::model::{hard_truncate_chars, single_line};
use crate::{CreatePr, ForgeError};

/// GitHub's PR title limit, and the shorter title retried when GitHub
/// rejects one as too long.
pub(crate) const TITLE_MAX: usize = 256;
pub(crate) const TITLE_RETRY_MAX: usize = 80;

/// `gh pr create` argv; `draft` adds `--draft`.
pub(crate) fn create_args(
    branch: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "pr", "create", "--head", branch, "--base", base, "--title", title, "--body", body,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if draft {
        args.push("--draft".to_owned());
    }
    args
}

/// The PR number from `gh pr create` stdout, else the already-open PR for
/// the head (`existing_head`).
pub(crate) fn create_number(create_out: &str, existing_head: i64) -> i64 {
    let n = pr_number_from_url(create_out.trim());
    if n > 0 { n } else { existing_head.max(0) }
}

/// `gh pr list` argv for the open PR on `branch` (gh pr view takes the
/// branch positionally and has no `--head`).
pub(crate) fn head_lookup_args(branch: &str, base: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "pr", "list", "--head", branch, "--state", "open", "--limit", "1", "--json", "number",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    let base = base.trim();
    if !base.is_empty() {
        args.extend(["--base".to_owned(), base.to_owned()]);
    }
    args
}

/// The first `number` in a `gh pr list --json number` answer, or 0.
pub(crate) fn list_first_number(raw: &str) -> i64 {
    let mut raw = raw.trim();
    if let Some(i) = raw.find('[') {
        raw = &raw[i..];
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Row {
        number: i64,
    }
    match decode::<Vec<Row>>(raw) {
        Ok(rows) => rows
            .first()
            .map(|r| r.number)
            .filter(|&n| n > 0)
            .unwrap_or(0),
        Err(_) => 0,
    }
}

/// Why a GitHub PR could not be opened, keeping the push error that is
/// often the real cause.
pub(crate) fn fail_reason(push_err: Option<&CmdError>, create_err: Option<&CmdError>) -> String {
    let mut b = String::new();
    if let Some(p) = push_err {
        b.push_str("git push failed: ");
        b.push_str(&strip_cmd_wrap(&p.to_string()));
        if create_err.is_some() {
            b.push_str("; ");
        }
    }
    if let Some(c) = create_err {
        b.push_str("gh pr create failed: ");
        b.push_str(&strip_cmd_wrap(&c.to_string()));
    }
    b
}

/// The display form of a create failure: command output only, so a long
/// `--body` argv is not what a length cap keeps.
pub(crate) fn fail_diag(s: &str) -> String {
    const CREATE: &str = "gh pr create failed: ";
    const PUSH: &str = "git push failed: ";
    let s = s.trim();
    let joined = format!("; {CREATE}");
    if let Some(i) = s.find(&joined) {
        let push = s[..i].strip_prefix(PUSH).unwrap_or(&s[..i]);
        let create = &s[i + joined.len()..];
        return format!(
            "{PUSH}{}; {CREATE}{}",
            strip_cmd_wrap(push),
            strip_cmd_wrap(create)
        );
    }
    if let Some(rest) = s.strip_prefix(CREATE) {
        return format!("{CREATE}{}", strip_cmd_wrap(rest));
    }
    if let Some(rest) = s.strip_prefix(PUSH) {
        return format!("{PUSH}{}", strip_cmd_wrap(rest));
    }
    strip_cmd_wrap(s)
}

/// The output tail of a `gh <argv>: <out>` / `git <argv>: <out>` error. The
/// last `": "` is the wrap; argv like `HEAD:refs/heads/x` has no space.
pub(crate) fn strip_cmd_wrap(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with("gh ") || s.starts_with("git "))
        && let Some(i) = s.rfind(": ")
    {
        let out = s[i + 2..].trim();
        if !out.is_empty() {
            return out.to_owned();
        }
    }
    s.to_owned()
}

impl GitHub {
    /// Go `CreateGitHubPR`. `Err` is the user-facing reason (the caller falls
    /// back to a Local PR).
    pub(crate) async fn create(&self, req: CreatePr<'_>) -> Result<i64, String> {
        if !self.cli_available().await {
            return Err(
                "gh CLI is not installed or not authenticated — using a Local PR instead".into(),
            );
        }
        if req.branch.trim().is_empty() {
            return Err("no branch to push — using a Local PR instead".into());
        }
        let mut title = hard_truncate_chars(&single_line(req.title), TITLE_MAX);
        if title.is_empty() {
            title = "Workspace changes".to_owned();
        }
        let push_dir = if req.worktree.as_os_str().is_empty() {
            req.repo
        } else {
            req.worktree
        };
        let push_err = git(
            push_dir,
            &[
                "push",
                "-u",
                "origin",
                &format!("HEAD:refs/heads/{}", req.branch),
            ],
        )
        .await
        .err();
        let mut res = self
            .gh
            .run(
                req.repo,
                &create_args(req.branch, req.base, &title, req.body, req.draft),
            )
            .await;
        // Match gh's own output: the error text also wraps --title/--body argv.
        if let Err(e) = &res
            && e.out.to_lowercase().contains("title is too long")
        {
            title = hard_truncate_chars(&title, TITLE_RETRY_MAX);
            res = self
                .gh
                .run(
                    req.repo,
                    &create_args(req.branch, req.base, &title, req.body, req.draft),
                )
                .await;
        }
        let out = match &res {
            Ok(out) => out.as_str(),
            Err(e) => e.out.as_str(),
        };
        let mut n = create_number(out, 0);
        if n <= 0 && push_err.is_none() {
            n = self
                .pr_number_for_head(req.repo, req.branch, req.base)
                .await;
        }
        if n > 0 && push_err.is_none() {
            return Ok(n);
        }
        let create_err = res.err();
        if push_err.is_some() || create_err.is_some() {
            return Err(fail_reason(push_err.as_ref(), create_err.as_ref()));
        }
        Err("gh pr create did not print a PR URL".into())
    }

    /// Go `githubPRNumberForHead`: the open PR for head/base, or 0.
    pub(crate) async fn pr_number_for_head(&self, repo: &Path, branch: &str, base: &str) -> i64 {
        if branch.trim().is_empty() {
            return 0;
        }
        match self.gh.run(repo, &head_lookup_args(branch, base)).await {
            Ok(out) => list_first_number(&out),
            Err(_) => 0,
        }
    }

    /// Go `pushGitHubHead`: pushes HEAD to the remote head an open PR tracks.
    /// `force` uses `--force-with-lease`. A fork PR pushes to the head repo,
    /// leases against the fetched PR head, and advances that ref.
    pub(crate) async fn push(
        &self,
        worktree: &Path,
        repo: &Path,
        pr: &Pr,
        remote_head: &str,
        force: bool,
    ) -> Result<(), ForgeError> {
        if remote_head.trim().is_empty() {
            return Err(ForgeError::invalid("empty remote head"));
        }
        let push_dir = if worktree.as_os_str().is_empty() {
            repo
        } else {
            worktree
        };
        let mut remote = "origin".to_owned();
        let mut lease = "--force-with-lease".to_owned();
        let pr_ref = super::head_ref(pr.number).local_ref;
        if pr.is_cross_repository {
            remote = self.head_repo_url(repo, pr.number).await.map_err(|e| {
                ForgeError::failed(format!(
                    "could not resolve the fork for PR #{}: {e}",
                    pr.number
                ))
            })?;
            if force {
                let sha = git(push_dir, &["rev-parse", "--verify", &pr_ref])
                    .await
                    .map_err(|e| {
                        ForgeError::failed(format!(
                            "no fetched head for PR #{} to lease against: {e}",
                            pr.number
                        ))
                    })?;
                lease = format!("{lease}=refs/heads/{remote_head}:{}", sha.trim());
            }
        }
        let mut args = vec!["push".to_owned()];
        if force {
            args.push(lease);
        }
        args.push(remote);
        args.push(format!("HEAD:refs/heads/{remote_head}"));
        git(push_dir, &args).await?;
        if pr.is_cross_repository {
            let _ = git(push_dir, &["update-ref", &pr_ref, "HEAD"]).await;
        }
        Ok(())
    }

    /// Go `githubPRHeadRepoURL`: a fork PR's push URL in origin's scheme.
    async fn head_repo_url(&self, repo: &Path, number: i64) -> Result<String, ForgeError> {
        let out = self
            .gh
            .run(
                repo,
                &[
                    "pr".to_owned(),
                    "view".to_owned(),
                    number.to_string(),
                    "--json".to_owned(),
                    "headRepositoryOwner,headRepository".to_owned(),
                ],
            )
            .await?;
        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Head {
            head_repository_owner: Login,
            head_repository: Name,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Login {
            login: String,
        }
        #[derive(Default, Deserialize)]
        #[serde(default)]
        struct Name {
            name: String,
        }
        let head = decode::<Head>(out.trim())
            .ok()
            .filter(|h| {
                !h.head_repository_owner.login.is_empty() && !h.head_repository.name.is_empty()
            })
            .ok_or_else(|| {
                ForgeError::failed(format!("gh pr view {number} returned no head repository"))
            })?;
        let origin = git(repo, &["remote", "get-url", "origin"]).await?;
        fork_remote_url(
            &origin,
            &head.head_repository_owner.login,
            &head.head_repository.name,
        )
        .ok_or_else(|| {
            ForgeError::failed(format!(
                "cannot derive a fork URL from origin {:?}",
                origin.trim()
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_args_draft_flag() {
        assert!(
            !create_args("feat", "main", "Title", "Body", false).contains(&"--draft".to_owned())
        );
        assert!(create_args("feat", "main", "Title", "Body", true).contains(&"--draft".to_owned()));
    }

    #[test]
    fn create_number_prefers_url_then_head() {
        assert_eq!(
            create_number("https://github.com/owner/repo/pull/482", 99),
            482
        );
        assert_eq!(create_number("not a url", 39), 39);
        assert_eq!(create_number("not a url", 0), 0);
    }

    #[test]
    fn head_lookup_lists_open_head_base() {
        let got = head_lookup_args("feat", "main");
        assert_eq!(&got[..2], ["pr", "list"]);
        let joined = got.join(" ");
        for flag in ["--head feat", "--base main", "--state open"] {
            assert!(joined.contains(flag), "{joined}");
        }
        assert_eq!(list_first_number(r#"[{"number":42}]"#), 42);
        assert_eq!(list_first_number("[]"), 0);
        assert_eq!(list_first_number("not json"), 0);
    }

    fn cmd(program: &'static str, args: &str, out: &str) -> CmdError {
        CmdError {
            program,
            args: args.into(),
            out: out.into(),
        }
    }

    #[test]
    fn fail_reason_keeps_push_error() {
        let create = cmd("gh", "pr create", "some create failure");
        assert_eq!(
            fail_reason(None, Some(&create)),
            "gh pr create failed: some create failure"
        );
        let push = cmd("git", "push", "permission denied");
        let got = fail_reason(Some(&push), Some(&create));
        assert!(
            got.contains("permission denied") && got.contains("some create failure"),
            "{got}"
        );
    }

    #[test]
    fn fail_diag_keeps_command_output() {
        let reason = fail_reason(
            Some(&cmd(
                "git",
                "push -u origin HEAD:refs/heads/feat",
                "rejected",
            )),
            Some(&cmd(
                "gh",
                "pr create --head feat --body a: long: body",
                "HTTP 401",
            )),
        );
        assert_eq!(
            fail_diag(&reason),
            "git push failed: rejected; gh pr create failed: HTTP 401"
        );
        assert_eq!(
            fail_diag("gh pr create failed: gh pr create --title x: boom"),
            "gh pr create failed: boom"
        );
        assert_eq!(fail_diag("other"), "other");
        assert_eq!(
            strip_cmd_wrap("git push origin HEAD:refs/heads/x: denied"),
            "denied"
        );
    }
}
