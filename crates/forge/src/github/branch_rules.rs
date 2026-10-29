//! Whether a branch requires a pull request: branch rulesets, then classic
//! branch protection.

use std::path::Path;

use serde::Deserialize;

use super::GitHub;
use super::repo::split_owner_repo;
use super::snapshot::flatten_json_array_pages;
use crate::cli::CmdError;
use crate::json::decode;

/// Go `url.PathEscape` for one path segment.
pub(crate) fn path_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'~' | b'$' | b'&' | b'+' | b'=' | b':' | b'@'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A `pull_request` rule in the branch's rulesets. `None` when the answer
/// does not parse.
pub(crate) fn rules_require_pr(raw: &str) -> Option<bool> {
    let mut raw = raw.trim();
    if let Some(i) = raw.find('[')
        && let Some(j) = raw.rfind(']')
        && j > i
    {
        raw = &raw[i..=j];
    }
    if raw.is_empty() {
        return None;
    }
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Rule {
        #[serde(rename = "type")]
        kind: String,
    }
    let rules: Vec<Rule> = decode(raw).ok()?;
    Some(
        rules
            .iter()
            .any(|r| r.kind.trim().eq_ignore_ascii_case("pull_request")),
    )
}

/// Classic protection with required reviews. A 404 means unprotected.
pub(crate) fn classic_protection_requires_pr(raw: &str, err: Option<&CmdError>) -> Option<bool> {
    if let Some(e) = err {
        return http_not_found(&e.to_string()).then_some(false);
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    #[derive(Default, Deserialize)]
    struct Protection {
        #[serde(default)]
        required_pull_request_reviews: Option<serde::de::IgnoredAny>,
    }
    // Decoded without null stripping: an explicit null means no reviews.
    let p: Protection = serde_json::from_str(raw).ok()?;
    Some(p.required_pull_request_reviews.is_some())
}

pub(crate) fn http_not_found(err: &str) -> bool {
    err.contains("(HTTP 404)")
}

/// Go `githubBranchRequiresPRFromAPIs`: required when either source says
/// so; not required only when both answered.
pub(crate) fn requires_pr_from_apis(
    rules_raw: &str,
    rules_err: Option<&CmdError>,
    prot_raw: &str,
    prot_err: Option<&CmdError>,
) -> Option<bool> {
    let rules = if rules_err.is_none() {
        rules_require_pr(&flatten_json_array_pages(rules_raw))
    } else {
        None
    };
    if rules == Some(true) {
        return Some(true);
    }
    let classic = classic_protection_requires_pr(prot_raw, prot_err);
    if classic == Some(true) {
        return Some(true);
    }
    (rules.is_some() && classic.is_some()).then_some(false)
}

impl GitHub {
    /// Go `fetchGitHubBranchRequiresPR`. `None` means unknown.
    pub(crate) async fn fetch_branch_requires_pr(
        &self,
        repo: &Path,
        owner_repo: &str,
        branch: &str,
    ) -> Option<bool> {
        let (owner, name) = split_owner_repo(owner_repo)?;
        let escaped = path_escape(branch);
        let rules_path = format!("repos/{owner}/{name}/rules/branches/{escaped}");
        let rules = self
            .gh
            .run(repo, &["api", "--paginate", "--slurp", &rules_path])
            .await;
        let (rules_out, rules_err) = match rules {
            Ok(out) => (out, None),
            Err(e) => (e.out.clone(), Some(e)),
        };
        if rules_err.is_none()
            && rules_require_pr(&flatten_json_array_pages(&rules_out)) == Some(true)
        {
            return Some(true);
        }
        let prot_path = format!("repos/{owner}/{name}/branches/{escaped}/protection");
        let (prot_out, prot_err) = match self.gh.run(repo, &["api", &prot_path]).await {
            Ok(out) => (out, None),
            Err(e) => (e.out.clone(), Some(e)),
        };
        requires_pr_from_apis(&rules_out, rules_err.as_ref(), &prot_out, prot_err.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(out: &str) -> CmdError {
        CmdError {
            program: "gh",
            args: "api".into(),
            out: out.into(),
        }
    }

    #[test]
    fn rules_require_pr_cases() {
        assert_eq!(
            rules_require_pr(
                r#"[{"type":"pull_request","parameters":{"required_approving_review_count":1}}]"#
            ),
            Some(true)
        );
        assert_eq!(
            rules_require_pr(r#"[{"type":"deletion"},{"type":"non_fast_forward"}]"#),
            Some(false)
        );
        assert_eq!(rules_require_pr("[]"), Some(false));
        assert_eq!(rules_require_pr("not json"), None);
        assert_eq!(
            rules_require_pr(&flatten_json_array_pages(
                r#"[[{"type":"deletion"}],[{"type":"pull_request"}]]"#
            )),
            Some(true)
        );
    }

    #[test]
    fn classic_protection_cases() {
        assert_eq!(
            classic_protection_requires_pr(
                r#"{"required_pull_request_reviews":{"required_approving_review_count":1}}"#,
                None
            ),
            Some(true)
        );
        assert_eq!(
            classic_protection_requires_pr(r#"{"required_status_checks":{"strict":true}}"#, None),
            Some(false)
        );
        assert_eq!(
            classic_protection_requires_pr(r#"{"required_pull_request_reviews":null}"#, None),
            Some(false)
        );
        assert_eq!(
            classic_protection_requires_pr("", Some(&err("gh: Not Found (HTTP 404)"))),
            Some(false)
        );
        assert_eq!(
            classic_protection_requires_pr("", Some(&err("network down"))),
            None
        );
    }

    #[test]
    fn from_apis_cases() {
        let not_found = err("gh: Not Found (HTTP 404)");
        assert_eq!(
            requires_pr_from_apis("[]", None, "", Some(&not_found)),
            Some(false)
        );
        assert_eq!(
            requires_pr_from_apis("[]", None, r#"{"required_pull_request_reviews":{}}"#, None),
            Some(true)
        );
        assert_eq!(
            requires_pr_from_apis(
                r#"[[{"type":"deletion"}],[{"type":"pull_request"}]]"#,
                None,
                "",
                Some(&not_found)
            ),
            Some(true)
        );
        assert_eq!(
            requires_pr_from_apis("[]", None, "", Some(&err("timeout"))),
            None
        );
    }

    #[test]
    fn path_escape_like_go() {
        assert_eq!(path_escape("feat/a b"), "feat%2Fa%20b");
        assert_eq!(path_escape("release-1.2_x~"), "release-1.2_x~");
    }
}
