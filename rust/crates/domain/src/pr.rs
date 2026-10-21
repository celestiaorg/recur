//! Pull requests: one review UI for GitHub and Local (worktree-diff) PRs.

use serde::{Deserialize, Serialize};

use crate::serde_util::{is_false, is_zero};
use crate::text::eq_fold;
use crate::{DiffStat, ImpactItem};

string_enum! {
    /// Distinguishes a GitHub PR from a Local (worktree-diff) PR (PRD §12).
    PrSource {
        GitHub = "gh",
        Local = "local",
    }
}

// Attention reason codes on PR.attention_reasons. The dashboard and PR page
// map these to labels so "Needs your attention" is never a mystery bucket.
pub const ATTENTION_REVIEW_REQUESTED: &str = "review_requested";
pub const ATTENTION_ASSIGNED: &str = "assigned";
pub const ATTENTION_CHANGES_REQUESTED: &str = "changes_requested";
pub const ATTENTION_CHECKS_FAILING: &str = "checks_failing";
/// Legacy alias of ATTENTION_REVIEW_REQUESTED (local PRs that need the
/// viewer). The UI treats both as "Needs your review".
pub const ATTENTION_AWAITING_REVIEW: &str = "awaiting_review";

// Review states on PrReviewer.state.
pub const REVIEW_STATE_PENDING: &str = "pending";
pub const REVIEW_STATE_APPROVED: &str = "approved";
pub const REVIEW_STATE_CHANGES_REQUESTED: &str = "changes_requested";
pub const REVIEW_STATE_COMMENTED: &str = "commented";

/// The human review boundary (PRD §12).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Pr {
    pub id: String,
    pub workspace_id: String,
    /// The project whose repo this PR belongs to. Set even when WorkspaceID
    /// is empty (a GitHub PR Recur imported, not opened).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub project_id: String,
    /// you/loop/open/merged for the PR list.
    pub group: String,
    pub number: i64,
    pub source: PrSource,
    /// The GitHub pull request page; empty for local PRs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The GitHub PR head commit (headRefOid); empty for local PRs.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub head_sha: String,
    /// A GitHub draft PR. Stored on Local fallbacks so a retry keeps intent.
    #[serde(skip_serializing_if = "is_false")]
    pub draft: bool,
    pub branch: String,
    pub base: String,
    pub title: String,
    /// `None` is Go's nil slice (`null`).
    pub summary: Option<Vec<SummaryPara>>,
    pub focus: FocusNote,
    /// "In review" / "Merged".
    pub state: String,
    pub diff: DiffStat,
    pub file_count: i64,
    /// True when nothing has failed: checks passed, are still running, or
    /// have not been reported yet.
    pub checks_ok: bool,
    /// True when at least one check run was reported.
    #[serde(skip_serializing_if = "is_false")]
    pub has_checks: bool,
    pub when: String,
    /// When the PR was opened (RFC3339).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    /// The last activity time (RFC3339).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
    /// `None` is Go's nil slice (`null`).
    pub commits: Option<Vec<Commit>>,
    /// `None` is Go's nil slice (`null`).
    pub files: Option<Vec<FileDiff>>,
    /// `None` is Go's nil slice (`null`).
    pub comments: Option<Vec<PrComment>>,
    /// Line notes not yet submitted with a review.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pending_comments: Vec<PendingLineComment>,
    /// `None` is Go's nil slice (`null`).
    pub verify: Option<Vec<VerifyStep>>,
    /// `None` is Go's nil slice (`null`).
    pub checks: Option<Vec<Check>>,
    /// How many commits the base is ahead of the PR branch (0 = up to date).
    #[serde(skip_serializing_if = "is_zero")]
    pub behind_base: i64,
    /// "" = idle, "git" = merge/rebase running, "coder" = conflict Coder turn.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updating_base: String,
    /// The observatory blast-radius for this PR.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub impact: Vec<ImpactItem>,
    /// The GitHub user who opened the PR.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author_login: String,
    /// Author, assignees, reviewers, and commenters. Deduped; author first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub participants: Vec<PrParticipant>,
    /// People asked to review, or who already reviewed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reviewers: Vec<PrReviewer>,
    /// The authenticated gh user looking at this PR.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub viewer_login: String,
    /// The viewer has push/maintain/admin on the repo.
    #[serde(skip_serializing_if = "is_false")]
    pub viewer_can_write: bool,
    /// The GitHub GraphQL id (used to react on the description).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    /// MERGEABLE, CONFLICTING, or UNKNOWN.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mergeable: String,
    /// CLEAN, DIRTY, BLOCKED, BEHIND, UNSTABLE, HAS_HOOKS, UNKNOWN.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub merge_state_status: String,
    /// APPROVED, CHANGES_REQUESTED, REVIEW_REQUIRED, or empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub review_decision: String,
    /// GitHub has an outstanding review request for the viewer.
    #[serde(skip_serializing_if = "is_false")]
    pub review_requested: bool,
    /// Why this PR is in "Needs your attention" (ATTENTION_* codes).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attention_reasons: Vec<String>,
    /// The PR head lives in a fork.
    #[serde(skip_serializing_if = "is_false")]
    pub is_cross_repository: bool,
    /// The repo's enabled merge types. Empty means unknown: show all three.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed_merge_methods: Vec<String>,
    /// The base branch requires a merge queue, so Merge enqueues.
    #[serde(skip_serializing_if = "is_false")]
    pub merge_queue_enabled: bool,
    /// In GitHub's merge queue or waiting on auto-merge to join it.
    #[serde(skip_serializing_if = "is_false")]
    pub merge_queued: bool,
}

impl Pr {
    fn is_github(&self) -> bool {
        self.source == PrSource::GitHub
    }

    /// Reports whether the authenticated gh user opened this PR.
    pub fn viewer_is_author(&self) -> bool {
        !self.viewer_login.is_empty() && eq_fold(&self.viewer_login, &self.author_login)
    }

    /// Whether the viewer can change the title and body. Local PRs are
    /// editable while open; GitHub needs author or write access. Merged and
    /// closed PRs are locked.
    pub fn can_edit_details(&self) -> bool {
        if self.conversation_locked() {
            return false;
        }
        !self.is_github() || self.viewer_is_author() || self.viewer_can_write
    }

    /// True for merged or closed PRs: no new comments or reviews.
    pub fn conversation_locked(&self) -> bool {
        let s = self.state.trim().to_lowercase();
        self.group == "merged" || s.contains("merged") || s.contains("closed")
    }

    /// Whether Approve / Request changes should be offered. GitHub forbids
    /// reviewing your own PR. Local PRs and an unknown viewer still offer it;
    /// write access is required otherwise.
    pub fn can_approve_review(&self) -> bool {
        if self.conversation_locked() {
            return false;
        }
        if !self.is_github() {
            return true;
        }
        if self.viewer_is_author() {
            return false;
        }
        self.viewer_login.is_empty() || self.viewer_can_write
    }

    /// Whether the viewer can edit this thread row.
    pub fn can_edit_comment(&self, c: &PrComment) -> bool {
        if c.kind == "action" {
            return false;
        }
        if !self.is_github() {
            return c.author == "You";
        }
        !self.viewer_login.is_empty() && eq_fold(&self.viewer_login, &c.author_login)
    }

    /// Whether the viewer can delete this thread row. Line notes and
    /// conversation comments are deletable (own row, or any row with write
    /// access); submitted reviews are not.
    pub fn can_delete_comment(&self, c: &PrComment) -> bool {
        if matches!(c.kind.as_str(), "action" | "description" | "hidden") || c.is_review_card() {
            return false;
        }
        self.can_edit_comment(c) || (self.is_github() && self.viewer_can_write)
    }

    /// Whether the viewer can resolve a line-note thread (GitHub: PR author
    /// or write access; local: always).
    pub fn can_resolve_thread(&self) -> bool {
        !self.is_github() || self.viewer_can_write || self.viewer_is_author()
    }

    /// Whether the viewer can add or remove a reaction on this row: any
    /// reactable conversation item (description, comments, reviews, line
    /// notes), and an authenticated viewer on GitHub.
    pub fn can_react_comment(&self, c: &PrComment) -> bool {
        if c.kind == "hidden" || !c.commit_sha.trim().is_empty() {
            return false;
        }
        if c.kind == "action" && !c.is_review_card() {
            return false;
        }
        !self.is_github() || !self.viewer_login.is_empty()
    }
}

/// One block of the Maestro-written PR summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SummaryPara {
    /// p / bullets / decision.
    pub kind: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub html: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<String>,
}

/// The Maestro's "where to focus your review" callout (PRD §12.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FocusNote {
    pub title: String,
    /// May contain <code>.
    pub body: String,
}

/// An agent-authored commit; messages encode iteration/role (PRD §11).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Commit {
    pub hash: String,
    /// Full hash for copy; Hash stays short.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha: String,
    pub message: String,
    /// The message body after the subject.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    /// Git author or GitHub login.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// Descriptive, e.g. "Coder".
    pub author_role: String,
    pub agent_code: String,
    pub color: String,
    pub iter_label: String,
    pub diff: DiffStat,
    pub when: String,
    /// `None` is Go's nil slice (`null`).
    pub files: Option<Vec<FileDiff>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
    #[serde(skip_serializing_if = "is_false")]
    pub verified: bool,
    /// pass, fail, pending.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub check_status: String,
}

/// One changed file with its diff lines (PRD §12.2 Files changed).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    /// Set on renames (status r).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub old_path: String,
    /// m/a/d/r.
    pub status: String,
    pub diff: DiffStat,
    /// `None` is Go's nil slice (`null`).
    pub lines: Option<Vec<DiffLine>>,
}

/// A files-changed list. Truncated means untracked paths past the scan cap
/// were omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileDiffResult {
    /// `None` is Go's nil slice (`null`).
    pub files: Option<Vec<FileDiff>>,
    #[serde(skip_serializing_if = "is_false")]
    pub truncated: bool,
}

/// One rendered diff row. CodeHTML/ExtraHTML hold pre-highlighted markup.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DiffLine {
    pub old: String,
    pub new: String,
    /// ctx/add/del/hunk.
    pub r#type: String,
    pub code_html: String,
    /// Inline comment / risk flag block.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub extra_html: String,
}

/// One unsaved inline note. It posts with SubmitReview (or SendBackToLoop),
/// not when the reviewer types it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PendingLineComment {
    pub id: String,
    pub file: String,
    pub line: i64,
    /// LEFT or RIGHT.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub side: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub start_line: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start_side: String,
    pub body: String,
    /// The observatory finding this draft was queued from, if any.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub finding_id: String,
}

/// One person on a pull request (dashboard avatar stack).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrParticipant {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_css: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
}

/// Someone asked to review this PR, or who already left a review.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrReviewer {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub login: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_css: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub code: String,
    /// pending, approved, changes_requested, or commented.
    pub state: String,
}

/// A review thread entry, human or agent (PRD §14 pr_comments).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PrComment {
    pub id: String,
    /// Avatar.
    pub code: String,
    pub author: String,
    /// The GitHub username when this row came from GH.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author_login: String,
    pub avatar_css: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub avatar_url: String,
    pub time: String,
    pub body_html: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub react_html: String,
    /// comment (default), action, or description.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// "commented", "approved", "requested changes". Empty on
    /// description/action rows.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub verb: String,
    /// Source markdown; the UI renders it when set.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body_md: String,
    /// The source timestamp (RFC3339) for ordering the GH timeline.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub at: String,
    /// A GH commit timeline row.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub commit_sha: String,
    #[serde(skip_serializing_if = "is_false")]
    pub verified: bool,
    /// pass, fail, pending.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub check_status: String,
    /// The GitHub permalink for this row.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Nests this row under another conversation card.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub parent_id: String,
    /// The GitHub pull_request_review_id for an inline comment.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub review_id: String,
    /// File context for a line note.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(skip_serializing_if = "is_zero")]
    pub line: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub side: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub diff_hunk: String,
    /// The first line of a GitHub multiline comment.
    #[serde(skip_serializing_if = "is_zero")]
    pub start_line: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub start_side: String,
    /// GitHub nulled `line`: Line/StartLine hold original_line for the label
    /// only; do not anchor to the live Files-changed rows.
    #[serde(skip_serializing_if = "is_false")]
    pub outdated: bool,
    /// The line-note thread this reply belongs to.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub in_reply_to_id: String,
    /// The GitHub review-thread node (PRRT_…) or the local root id.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thread_id: String,
    #[serde(skip_serializing_if = "is_false")]
    pub resolved: bool,
    /// The GitHub GraphQL id for addReaction/removeReaction.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub node_id: String,
    /// The emoji rollups on this row (count > 0).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<PrReaction>,
}

impl PrComment {
    /// A file-anchored review comment (or a reply on one).
    pub fn is_line_note(&self) -> bool {
        self.id.starts_with("gh-inline-")
            || !self.path.trim().is_empty()
            || !self.in_reply_to_id.trim().is_empty()
            || !self.diff_hunk.trim().is_empty()
    }

    /// A submitted pull-request review (body or action row).
    pub fn is_review_card(&self) -> bool {
        self.id.starts_with("gh-review-")
    }
}

/// One GitHub reaction emoji and its rollup on a conversation item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrReaction {
    /// The REST reaction name: +1, -1, laugh, hooray, confused, heart,
    /// rocket, eyes.
    pub content: String,
    pub count: i64,
    #[serde(skip_serializing_if = "is_false")]
    pub mine: bool,
}

/// One browser-verification evidence row (PRD §10). Screenshot is the
/// on-disk path the QA agent captured for this step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VerifyStep {
    pub html: String,
    pub passed: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub screenshot: String,
}

/// One entry in the checks rollup: Recur review/verify/typecheck rows for
/// Local PRs, the GitHub Checks tab for GitHub PRs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Check {
    pub name: String,
    pub detail: String,
    pub passed: bool,
    /// pass, fail, pending, skipping, cancel. Empty for Local Recur checks.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
    /// The GitHub check-run page (pending rows prefer the PR Checks tab).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// The check-run start (RFC3339) the UI ticks pending elapsed time from.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub started_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gh(viewer: &str, author: &str, write: bool) -> Pr {
        Pr {
            source: PrSource::GitHub,
            viewer_login: viewer.into(),
            author_login: author.into(),
            viewer_can_write: write,
            ..Default::default()
        }
    }

    fn comment(id: &str, kind: &str) -> PrComment {
        PrComment {
            id: id.into(),
            kind: kind.into(),
            ..Default::default()
        }
    }

    #[test]
    fn can_react_comment() {
        let gh_pr = gh("avery", "", false);
        let anon = gh("", "", false);
        let local = Pr {
            source: PrSource::Local,
            ..Default::default()
        };
        let c = comment("gh-comment-1", "");
        let review = comment("gh-review-action-1", "action");
        let merge = comment("gh-event-merged", "action");
        let commit = PrComment {
            commit_sha: "abc".into(),
            ..comment("gh-event-c", "action")
        };
        let desc = comment("gh-description", "description");
        assert!(
            gh_pr.can_react_comment(&c)
                && gh_pr.can_react_comment(&review)
                && gh_pr.can_react_comment(&desc)
        );
        assert!(!gh_pr.can_react_comment(&merge) && !gh_pr.can_react_comment(&commit));
        assert!(!gh_pr.can_react_comment(&comment("", "hidden")));
        assert!(!anon.can_react_comment(&c));
        assert!(local.can_react_comment(&c) && !local.can_react_comment(&merge));
    }

    #[test]
    fn conversation_lock_and_edit_details() {
        let merged = Pr {
            group: "merged".into(),
            ..Default::default()
        };
        assert!(merged.conversation_locked() && !merged.can_edit_details());
        let closed = Pr {
            state: " Closed ".into(),
            ..Default::default()
        };
        assert!(closed.conversation_locked() && !closed.can_approve_review());
        assert!(
            Pr::default().can_edit_details(),
            "local PRs are editable while open"
        );
        assert!(
            gh("Avery", "avery", false).can_edit_details(),
            "author, case-insensitive"
        );
        assert!(gh("bo", "avery", true).can_edit_details());
        assert!(!gh("bo", "avery", false).can_edit_details());
    }

    #[test]
    fn approve_review_rules() {
        assert!(Pr::default().can_approve_review());
        assert!(
            !gh("avery", "avery", true).can_approve_review(),
            "never your own PR"
        );
        assert!(
            gh("", "avery", false).can_approve_review(),
            "unknown viewer still offered"
        );
        assert!(gh("bo", "avery", true).can_approve_review());
        assert!(!gh("bo", "avery", false).can_approve_review());
    }

    #[test]
    fn comment_edit_delete_resolve() {
        let local = Pr::default();
        let mine = PrComment {
            author: "You".into(),
            ..comment("c1", "")
        };
        assert!(local.can_edit_comment(&mine) && local.can_delete_comment(&mine));
        assert!(!local.can_edit_comment(&comment("c2", "")));
        assert!(!local.can_edit_comment(&comment("a", "action")));

        let pr = gh("bo", "avery", false);
        let own = PrComment {
            author_login: "BO".into(),
            ..comment("gh-comment-1", "")
        };
        assert!(pr.can_edit_comment(&own) && pr.can_delete_comment(&own));
        let other = PrComment {
            author_login: "cy".into(),
            ..comment("gh-comment-2", "")
        };
        assert!(!pr.can_delete_comment(&other));
        assert!(
            gh("bo", "avery", true).can_delete_comment(&other),
            "write access deletes any row"
        );
        let review = PrComment {
            author_login: "bo".into(),
            ..comment("gh-review-1", "")
        };
        assert!(
            !gh("bo", "avery", true).can_delete_comment(&review),
            "submitted reviews stay"
        );
        for kind in ["description", "hidden", "action"] {
            assert!(
                !gh("bo", "avery", true).can_delete_comment(&comment("x", kind)),
                "{kind}"
            );
        }

        assert!(Pr::default().can_resolve_thread());
        assert!(gh("bo", "avery", true).can_resolve_thread());
        assert!(gh("avery", "avery", false).can_resolve_thread());
        assert!(!gh("bo", "avery", false).can_resolve_thread());
        assert!(!gh("", "", false).can_resolve_thread());
    }

    #[test]
    fn comment_shapes() {
        assert!(comment("gh-inline-1", "").is_line_note());
        assert!(
            PrComment {
                path: "a.go".into(),
                ..Default::default()
            }
            .is_line_note()
        );
        assert!(
            PrComment {
                in_reply_to_id: "r".into(),
                ..Default::default()
            }
            .is_line_note()
        );
        assert!(
            PrComment {
                diff_hunk: "@@".into(),
                ..Default::default()
            }
            .is_line_note()
        );
        assert!(
            !PrComment {
                path: "  ".into(),
                ..Default::default()
            }
            .is_line_note()
        );
        assert!(comment("gh-review-9", "").is_review_card());
        assert!(!comment("gh-comment-9", "").is_review_card());
    }
}
