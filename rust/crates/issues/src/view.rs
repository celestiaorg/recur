//! Issue links for workspace chips (`lib/issue-link.ts`).

use std::sync::Arc;

use recur_domain::IssueRef;

use crate::{AssignedIssue, IssueTracker};

/// `source:id`, the key the UI dedupes tracker issues by.
pub fn issue_key(source: &str, id: &str) -> String {
    format!("{source}:{id}")
}

// simplify: the URL comes from the issue list, so a linked Linear issue that
// dropped off it (closed, reassigned) has no URL; store it on IssueRef if
// that matters.
/// The issue's web page: from the listed issues, else what its tracker can
/// derive from the key.
pub fn issue_url(
    issue: Option<&IssueRef>,
    issues: &[AssignedIssue],
    trackers: &[Arc<dyn IssueTracker>],
) -> String {
    let Some(r) = issue else {
        return String::new();
    };
    if let Some(hit) = issues
        .iter()
        .find(|i| i.source == r.source && i.id == r.id && !i.url.is_empty())
    {
        return hit.url.clone();
    }
    trackers
        .iter()
        .find(|t| t.id() == r.source)
        .and_then(|t| t.issue_url(r))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GitHubIssues;
    use crate::linear::Linear;

    #[test]
    fn urls_from_list_then_tracker() {
        let trackers: Vec<Arc<dyn IssueTracker>> =
            vec![Arc::new(GitHubIssues::new()), Arc::new(Linear::new())];
        let r = |source: &str, id: &str, key: &str| IssueRef {
            source: source.into(),
            id: id.into(),
            key: key.into(),
        };
        let listed = [AssignedIssue {
            source: "linear".into(),
            id: "u1".into(),
            url: "https://linear.app/x/issue/ENG-1".into(),
            ..Default::default()
        }];
        assert_eq!(
            issue_url(Some(&r("linear", "u1", "ENG-1")), &listed, &trackers),
            "https://linear.app/x/issue/ENG-1"
        );
        assert_eq!(
            issue_url(Some(&r("linear", "u2", "ENG-2")), &listed, &trackers),
            ""
        );
        assert_eq!(
            issue_url(Some(&r("github", "I_1", "acme/app#12")), &[], &trackers),
            "https://github.com/acme/app/issues/12"
        );
        assert_eq!(issue_url(None, &listed, &trackers), "");
        assert_eq!(issue_key("linear", "u1"), "linear:u1");
    }
}
