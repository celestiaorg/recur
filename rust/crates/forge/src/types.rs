//! Forge-neutral request and result types. Values that mirror a forge's own
//! vocabulary use GitHub's spelling, which is also what `recur_domain::Pr`
//! stores (`OPEN`/`MERGED`/`CLOSED`, `APPROVED`/`CHANGES_REQUESTED`/
//! `REVIEW_REQUIRED`, `MERGEABLE`/`CONFLICTING`/`UNKNOWN`, `CLEAN`/`DIRTY`/
//! `BEHIND`/…). Another forge maps its states onto these.

use std::collections::HashMap;
use std::path::Path;

use recur_domain::{Check, Commit, PrComment, PrParticipant, PrReviewer};

/// Which PRs a list call returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrListState {
    Open,
    /// Recently merged, newest activity first.
    Merged,
}

impl PrListState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Merged => "merged",
        }
    }

    /// Rows the PR sweep asks for (Go `githubOpenPRLimit` / `githubMergedPRLimit`).
    pub fn sweep_limit(self) -> usize {
        match self {
            Self::Open => 100,
            Self::Merged => 20,
        }
    }
}

/// One PR from a list call. Times are the forge's raw RFC3339 strings
/// (see [`crate::model::normalize_time`]); participants and reviewers are
/// already resolved with the forge's avatars.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrListRow {
    pub number: i64,
    pub title: String,
    pub url: String,
    pub head_ref: String,
    pub base_ref: String,
    pub author_login: String,
    pub is_draft: bool,
    pub is_cross_repository: bool,
    pub created_at: String,
    pub updated_at: String,
    pub additions: i64,
    pub deletions: i64,
    pub changed_files: i64,
    pub review_decision: String,
    pub state: String,
    pub mergeable: String,
    pub merge_state_status: String,
    pub review_requests: Vec<PrParticipant>,
    pub assignees: Vec<PrParticipant>,
    /// Check rows from the list's rollup; empty when none were reported.
    pub checks: Vec<Check>,
    /// Nothing in the rollup failed.
    pub checks_ok: bool,
    /// The rollup reported at least one check.
    pub has_checks: bool,
    pub participants: Vec<PrParticipant>,
    pub reviewers: Vec<PrReviewer>,
}

/// State probe for a PR missing from list results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrStateProbe {
    /// Upper-case `OPEN`, `MERGED` or `CLOSED`.
    pub state: String,
    /// Normalized RFC3339 UTC, or empty.
    pub updated_at: String,
}

/// Author identity for one commit, ready to stamp onto a [`Commit`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitAuthor {
    /// Display name for `Commit.author`.
    pub name: String,
    pub avatar_url: String,
}

/// Signature and check rollup for one commit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitMeta {
    pub verified: bool,
    /// pass, fail, pending, or empty.
    pub check_status: String,
    pub url: String,
}

/// The live PR conversation: one forge snapshot that the engine merges into
/// the stored PR.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrSnapshot {
    pub title: String,
    pub state: String,
    pub review_decision: String,
    pub mergeable: String,
    pub merge_state_status: String,
    pub is_draft: bool,
    pub head_sha: String,
    pub head_ref: String,
    pub base_ref: String,
    pub url: String,
    pub author_login: String,
    /// The author's avatar, resolved.
    pub author_avatar_url: String,
    pub viewer_login: String,
    pub viewer_can_write: bool,
    pub allowed_merge_methods: Vec<String>,
    pub merge_queue_enabled: bool,
    pub merge_queued: bool,
    pub node_id: String,
    /// Opened-at (RFC3339).
    pub created_at: String,
    /// Last activity (RFC3339).
    pub updated_at: String,
    pub review_requests: Vec<PrParticipant>,
    pub assignees: Vec<PrParticipant>,
    /// Description, comments, reviews, line notes and timeline rows.
    pub comments: Vec<PrComment>,
    /// The PR's commits as the forge lists them, newest last.
    pub commits: Vec<Commit>,
    /// Commit sha → linked forge user.
    pub commit_authors: HashMap<String, CommitAuthor>,
    /// Commit sha → signature and checks.
    pub commit_meta: HashMap<String, CommitMeta>,
    /// Lower-cased login or display name → avatar URL.
    pub author_avatars: HashMap<String, String>,
    // simplify: the snapshot reads the last 100 comments and reviews; these
    // flags tell the merge to keep older stored rows. Paginate those
    // connections if first loads of 100+ row threads matter.
    pub comments_truncated: bool,
    pub reviews_truncated: bool,
}

/// The authenticated user as the repo sees them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Viewer {
    pub login: String,
    pub can_write: bool,
    /// Enabled merge methods; empty means unknown.
    pub allowed_merge_methods: Vec<String>,
}

/// A PR's check rollup.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChecksReport {
    pub checks: Vec<Check>,
    /// Nothing failed and the rollup is complete.
    pub ok: bool,
}

/// Open a PR for a pushed branch.
#[derive(Debug, Clone, Copy)]
pub struct CreatePr<'a> {
    /// The project's checkout (forge CLI cwd).
    pub repo: &'a Path,
    /// Where to push from; empty means `repo`.
    pub worktree: &'a Path,
    pub branch: &'a str,
    /// The base as the forge names it (`main`, not `origin/main`).
    pub base: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub draft: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged,
    /// The base requires a merge queue; the PR was enqueued and stays open.
    Queued,
}

/// Where a PR head can be fetched from and kept locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadRef {
    /// Remote ref to fetch (`pull/N/head`).
    pub remote_ref: String,
    /// Local tracking ref (`refs/recur/pr/N`).
    pub local_ref: String,
}

/// The PR a chat is looking at, for agent-facing subject text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrSubject {
    pub number: i64,
    pub url: String,
    pub title: String,
    pub branch: String,
    pub base: String,
    /// The chat is on a PR chat host (the project's checkout), not a
    /// worktree of the head.
    pub host: bool,
}

/// Subject and body for a forge merge commit. `omit_body` leaves the body to
/// the forge (a squash with no commit list).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeCommitText {
    pub subject: String,
    pub body: String,
    pub omit_body: bool,
}

impl MergeCommitText {
    /// Subject, blank line, body.
    pub fn combined(&self) -> String {
        let body = self.body.trim();
        if body.is_empty() {
            return self.subject.clone();
        }
        format!("{}\n\n{}", self.subject, body)
    }
}

/// A fetched attachment image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageBytes {
    pub content_type: String,
    pub bytes: Vec<u8>,
}
