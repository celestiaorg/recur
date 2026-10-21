//! Repository identity: owner/repo from remotes, PR URLs and `gh repo view`.

use std::path::Path;

use serde::Deserialize;

use super::GitHub;
use crate::ForgeError;
use crate::cli::git;
use crate::json::decode;

/// `owner/repo` from an ssh or https remote URL (any host). Local paths and
/// URLs without two path segments are not remotes.
pub fn owner_repo_from_remote_url(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('/') || s.starts_with('.') {
        return None;
    }
    let path = if let Some(i) = s.find("://") {
        let rest = &s[i + 3..];
        &rest[rest.find('/')? + 1..]
    } else {
        &s[s.find(':')? + 1..]
    };
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some(format!(
        "{owner}/{}",
        repo.strip_suffix(".git").unwrap_or(repo)
    ))
}

/// Replaces the trailing owner/repo of a remote URL (`git@host:owner/repo.git`,
/// `https://host/owner/repo`) with `owner/repo.git`.
pub fn fork_remote_url(remote_url: &str, owner: &str, repo: &str) -> Option<String> {
    let s = remote_url.trim();
    let s = s.strip_suffix('/').unwrap_or(s);
    let s = s.strip_suffix(".git").unwrap_or(s);
    let i = s.rfind('/')?;
    if i == 0 {
        return None;
    }
    let j = s[..i].rfind(['/', ':'])?;
    if j == i - 1 {
        return None;
    }
    Some(format!("{}{owner}/{repo}.git", &s[..=j]))
}

/// `owner/repo` split at the first slash; both halves non-empty.
pub fn split_owner_repo(owner_repo: &str) -> Option<(String, String)> {
    let s = owner_repo.trim();
    let i = s.find('/')?;
    if i == 0 || i >= s.len() - 1 {
        return None;
    }
    Some((s[..i].to_owned(), s[i + 1..].to_owned()))
}

/// `owner/repo` from a PR page URL (`https://github.com/owner/repo/pull/1`).
pub fn owner_repo_from_pr_url(url: &str) -> Option<String> {
    let mut u = url.trim();
    u = u.strip_suffix('/').unwrap_or(u);
    for p in ["https://", "http://"] {
        u = u.strip_prefix(p).unwrap_or(u);
    }
    let parts: Vec<&str> = u.split('/').collect();
    if parts.len() < 3 || parts[1].is_empty() || parts[2].is_empty() {
        return None;
    }
    Some(format!("{}/{}", parts[1], parts[2]))
}

/// The trailing PR number of a PR URL; 0 when the tail is not numeric.
pub fn pr_number_from_url(url: &str) -> i64 {
    let url = url.trim_end_matches('/');
    let Some(i) = url.rfind('/') else {
        return 0;
    };
    let mut n: i64 = 0;
    for c in url[i + 1..].chars() {
        let Some(d) = c.to_digit(10) else {
            return 0;
        };
        n = n.wrapping_mul(10).wrapping_add(i64::from(d));
    }
    n
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct UrlOnly {
    url: String,
}

impl GitHub {
    /// Go `githubRepoOwnerName`: `gh repo view` (gh's default repo for the
    /// checkout), cached per checkout.
    pub(crate) async fn repo_owner_name(
        &self,
        repo: &Path,
    ) -> Result<(String, String), ForgeError> {
        if let Some(v) = self.owner_cache.lock().get(repo).cloned()
            && let Some(pair) = split_owner_repo(&v)
        {
            return Ok(pair);
        }
        let out = self
            .gh
            .run(repo, &["repo", "view", "--json", "nameWithOwner"])
            .await?;
        #[derive(Default, Deserialize)]
        #[serde(default, rename_all = "camelCase")]
        struct Meta {
            name_with_owner: String,
        }
        let meta: Meta = decode(out.trim())
            .map_err(|e| ForgeError::failed(format!("parse gh repo view: {e}")))?;
        let pair = split_owner_repo(&meta.name_with_owner)
            .ok_or_else(|| ForgeError::failed("missing owner/repo"))?;
        self.owner_cache
            .lock()
            .insert(repo.to_path_buf(), meta.name_with_owner);
        Ok(pair)
    }

    /// Go `githubPRViewURL`: the PR page URL, or empty.
    pub(crate) async fn pr_view_url(&self, repo: &Path, number: i64) -> String {
        let Ok(out) = self
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
            .await
        else {
            return String::new();
        };
        decode::<UrlOnly>(out.trim())
            .map(|m| m.url.trim().to_owned())
            .unwrap_or_default()
    }

    /// Go `ownerRepoForPR`.
    pub(crate) async fn owner_repo_for_pr(
        &self,
        repo: &Path,
        number: i64,
    ) -> Result<String, ForgeError> {
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
        let meta: UrlOnly =
            decode(out.trim()).map_err(|e| ForgeError::failed(format!("parse gh pr view: {e}")))?;
        owner_repo_from_pr_url(&meta.url)
            .ok_or_else(|| ForgeError::failed("missing repo for pull comment"))
    }

    /// Go `githubOwnerRepoForRemote`.
    pub(crate) async fn owner_repo_for_remote(
        &self,
        repo: &Path,
        remote: &str,
    ) -> Result<String, ForgeError> {
        let out = git(repo, &["remote", "get-url", remote]).await?;
        owner_repo_from_remote_url(&out)
            .ok_or_else(|| ForgeError::failed(format!("remote {remote} is not a GitHub URL")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_repo_from_remote_urls() {
        for (input, want) in [
            ("https://github.com/acme/app.git", Some("acme/app")),
            ("https://github.com/acme/app", Some("acme/app")),
            ("git@github.com:acme/app.git", Some("acme/app")),
            ("ssh://git@github.com/acme/parent.git", Some("acme/parent")),
            ("/tmp/origin.git", None),
            ("", None),
        ] {
            assert_eq!(
                owner_repo_from_remote_url(input).as_deref(),
                want,
                "{input}"
            );
        }
    }

    #[test]
    fn fork_urls_keep_scheme() {
        assert_eq!(
            fork_remote_url("git@github.com:acme/app.git", "bo", "app").as_deref(),
            Some("git@github.com:bo/app.git")
        );
        assert_eq!(
            fork_remote_url("https://github.com/acme/app/", "bo", "fork").as_deref(),
            Some("https://github.com/bo/fork.git")
        );
        assert_eq!(fork_remote_url("app", "bo", "app"), None);
    }

    #[test]
    fn pr_urls() {
        assert_eq!(
            owner_repo_from_pr_url("https://github.com/acme/app/pull/12").as_deref(),
            Some("acme/app")
        );
        assert_eq!(owner_repo_from_pr_url("not-a-pr"), None);
        for (input, want) in [
            ("https://github.com/owner/repo/pull/482", 482),
            ("https://github.com/owner/repo/pull/1", 1),
            ("https://github.com/owner/repo/pull/", 0),
            ("not a url", 0),
            ("https://github.com/owner/repo/pull/abc", 0),
        ] {
            assert_eq!(pr_number_from_url(input), want, "{input}");
        }
        assert_eq!(split_owner_repo("a/b"), Some(("a".into(), "b".into())));
        assert_eq!(split_owner_repo("/b"), None);
        assert_eq!(split_owner_repo("a/"), None);
    }
}
