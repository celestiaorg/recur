//! Whether a branch requires a pull request: branch rulesets, then classic
//! branch protection.

use std::path::Path;

use serde::Deserialize;

use super::GitHub;
use super::repo::split_owner_repo;
use super::snapshot::flatten_json_array_pages;
use crate::cli::CmdError;
use crate::json::decode;

/// Percent-escapes one URL path segment.
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

/// The rulesets behind the branch's `pull_request` rules (`None` for a rule
/// without an id). `None` when the answer does not parse.
pub(crate) fn pr_rulesets(raw: &str) -> Option<Vec<Option<u64>>> {
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
        ruleset_id: Option<u64>,
    }
    let rules: Vec<Rule> = decode(raw).ok()?;
    Some(
        rules
            .into_iter()
            .filter(|r| r.kind.trim().eq_ignore_ascii_case("pull_request"))
            .map(|r| r.ruleset_id)
            .collect(),
    )
}

/// The viewer can push past the ruleset at any time, not only through a PR.
pub(crate) fn ruleset_bypass_always(raw: &str) -> bool {
    #[derive(Default, Deserialize)]
    #[serde(default)]
    struct Ruleset {
        current_user_can_bypass: String,
    }
    decode::<Ruleset>(raw.trim())
        .is_ok_and(|r| matches!(r.current_user_can_bypass.as_str(), "always" | "exempt"))
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

/// Required when either source says so; not required only when both answered.
pub(crate) fn requires_pr_from_apis(
    rules: Option<bool>,
    prot_raw: &str,
    prot_err: Option<&CmdError>,
) -> Option<bool> {
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
    /// `None` means unknown.
    pub(crate) async fn fetch_branch_requires_pr(
        &self,
        repo: &Path,
        owner_repo: &str,
        branch: &str,
    ) -> Option<bool> {
        let (owner, name) = split_owner_repo(owner_repo)?;
        let escaped = path_escape(branch);
        let rules_path = format!("repos/{owner}/{name}/rules/branches/{escaped}");
        let rules = match self
            .gh
            .run(repo, &["api", "--paginate", "--slurp", &rules_path])
            .await
        {
            Ok(out) => match pr_rulesets(&flatten_json_array_pages(&out)) {
                Some(ids) => Some(!self.bypasses_all(repo, &owner, &name, &ids).await),
                None => None,
            },
            Err(_) => None,
        };
        if rules == Some(true) {
            return Some(true);
        }
        let prot_path = format!("repos/{owner}/{name}/branches/{escaped}/protection");
        let (prot_out, prot_err) = match self.gh.run(repo, &["api", &prot_path]).await {
            Ok(out) => (out, None),
            Err(e) => (e.out.clone(), Some(e)),
        };
        requires_pr_from_apis(rules, &prot_out, prot_err.as_ref())
    }

    /// Every ruleset lets the viewer bypass it; an id-less rule or a failed
    /// lookup counts as binding.
    async fn bypasses_all(
        &self,
        repo: &Path,
        owner: &str,
        name: &str,
        ids: &[Option<u64>],
    ) -> bool {
        for id in ids {
            let Some(id) = id else { return false };
            let path = format!("repos/{owner}/{name}/rulesets/{id}");
            match self.gh.run(repo, &["api", &path]).await {
                Ok(out) if ruleset_bypass_always(&out) => {}
                _ => return false,
            }
        }
        true
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
        let ids = |raw: &str| pr_rulesets(raw).map(|v| v.len());
        assert_eq!(
            ids(r#"[{"type":"pull_request","parameters":{"required_approving_review_count":1}}]"#),
            Some(1)
        );
        assert_eq!(
            ids(r#"[{"type":"deletion"},{"type":"non_fast_forward"}]"#),
            Some(0)
        );
        assert_eq!(ids("[]"), Some(0));
        assert_eq!(ids("not json"), None);
        assert_eq!(
            ids(&flatten_json_array_pages(
                r#"[[{"type":"deletion"}],[{"type":"pull_request"}]]"#
            )),
            Some(1)
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
            requires_pr_from_apis(Some(false), "", Some(&not_found)),
            Some(false)
        );
        assert_eq!(
            requires_pr_from_apis(Some(false), r#"{"required_pull_request_reviews":{}}"#, None),
            Some(true)
        );
        assert_eq!(
            requires_pr_from_apis(Some(true), "", Some(&not_found)),
            Some(true)
        );
        assert_eq!(
            requires_pr_from_apis(Some(false), "", Some(&err("timeout"))),
            None
        );
        assert_eq!(requires_pr_from_apis(None, "", Some(&not_found)), None);
    }

    #[test]
    fn pr_rulesets_and_bypass() {
        assert_eq!(
            pr_rulesets(
                r#"[{"type":"deletion","ruleset_id":1},{"type":"pull_request","ruleset_id":7},{"type":"pull_request"}]"#
            ),
            Some(vec![Some(7), None])
        );
        assert!(ruleset_bypass_always(
            r#"{"name":"main","current_user_can_bypass":"always"}"#
        ));
        assert!(ruleset_bypass_always(
            r#"{"current_user_can_bypass":"exempt"}"#
        ));
        assert!(!ruleset_bypass_always(
            r#"{"current_user_can_bypass":"pull_requests_only"}"#
        ));
        assert!(!ruleset_bypass_always(
            r#"{"current_user_can_bypass":"never"}"#
        ));
        assert!(!ruleset_bypass_always("gh: Not Found (HTTP 404)"));
    }

    #[test]
    fn path_escape_like_go() {
        assert_eq!(path_escape("feat/a b"), "feat%2Fa%20b");
        assert_eq!(path_escape("release-1.2_x~"), "release-1.2_x~");
    }
}
