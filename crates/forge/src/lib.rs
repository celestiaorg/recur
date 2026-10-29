//! Remote git hosting platforms behind one [`Forge`] trait (pull requests,
//! reviews, checks, reactions). GitHub ships first ([`github::GitHub`], over
//! the `gh` CLI).
//!
//! A forge owns everything that differs between hosts: commands and APIs,
//! response parsing, comment id schemes, avatar and permalink rules, and the
//! user-facing wording about the host. The engine keeps the PR model and the
//! forge-neutral rules in [`model`]; the UI's pure PR rules are in [`view`].
//! Adding GitLab or Bitbucket is one module implementing [`Forge`] plus one
//! registration line in `recur-services`.

mod cli;
mod error;
pub mod github;
pub mod json;
pub mod model;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
mod types;
pub mod view;

use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use recur_domain::{
    MergeCommitMessage, MergeMethod, PendingLineComment, Pr, PrComment, PrParticipant, PrSource,
    SquashCommitMessage,
};

pub use cli::{CmdError, Gh, git, plain_stdout};
pub use error::{ForgeError, ForgeErrorKind};
pub use types::*;

/// One remote git host. A PR is handled by the forge whose
/// [`Forge::pr_source`] matches `Pr.source`; Local PRs have no forge.
///
/// `repo` is the project's checkout (the CLI's working directory). Errors
/// carry the user-facing text the Go app showed.
#[async_trait]
pub trait Forge: Send + Sync + 'static {
    /// Registry key (`github`).
    fn id(&self) -> &str;
    /// `Pr.source` of this forge's PRs (`gh`).
    fn pr_source(&self) -> PrSource;
    /// User-facing name (`GitHub`).
    fn label(&self) -> &str;
    /// `owner/repo` from a remote URL, or `None` when it is not one.
    fn detect(&self, remote_url: &str) -> Option<String>;
    /// Stable id for an imported PR row (`gh-<project>-<n>`).
    fn imported_pr_id(&self, project_id: &str, number: i64) -> String;
    /// Avatar URL for a login, preferring `provided`. The backend stores
    /// 56 px; the UI asks for 40 px for login-only people.
    fn avatar_url(&self, login: &str, provided: &str, size: u32) -> String;

    // ---- Availability and identity
    /// The CLI is installed and authenticated (cached 30 s).
    async fn available(&self) -> bool;
    /// The authenticated login, or empty.
    async fn login(&self) -> String;
    /// Login, write access and merge methods for `owner_repo`.
    async fn viewer(&self, repo: &Path, owner_repo: &str) -> Viewer;
    /// `owner/repo` of a configured git remote of the checkout.
    async fn remote_repo(&self, repo: &Path, remote: &str) -> Result<String, ForgeError>;

    // ---- PR sync
    async fn list_prs(
        &self,
        repo: &Path,
        state: PrListState,
        limit: usize,
    ) -> Result<Vec<PrListRow>, ForgeError>;
    /// State of a PR missing from list results.
    async fn pr_state(&self, repo: &Path, number: i64) -> Result<PrStateProbe, ForgeError>;
    /// The live conversation, review state and viewer.
    async fn snapshot(&self, repo: &Path, number: i64) -> Result<PrSnapshot, ForgeError>;
    /// Unified diff of the PR.
    async fn diff(&self, repo: &Path, number: i64) -> Result<String, ForgeError>;
    /// The check rollup. `pr_url` may be empty.
    async fn checks(
        &self,
        repo: &Path,
        number: i64,
        pr_url: &str,
    ) -> Result<ChecksReport, ForgeError>;
    /// Merges a snapshot's rows with stored ones, keeping Recur-only rows,
    /// rows the snapshot window cut off, and just-posted line notes.
    fn merge_comments(
        &self,
        existing: &[PrComment],
        snap: &PrSnapshot,
        now: DateTime<Utc>,
    ) -> Vec<PrComment>;

    // ---- Create, push, branches
    /// Pushes the branch and opens a PR. `Err` is the reason shown when the
    /// caller falls back to a Local PR.
    async fn create_pr(&self, req: CreatePr<'_>) -> Result<i64, String>;
    /// Pushes HEAD to the remote head an open PR tracks (forks included).
    async fn push_head(
        &self,
        worktree: &Path,
        repo: &Path,
        pr: &Pr,
        remote_head: &str,
        force: bool,
    ) -> Result<(), ForgeError>;
    /// Where a PR head is fetched from and kept locally.
    fn head_ref(&self, number: i64) -> HeadRef;
    /// Changes an open PR's base (`base` as the forge names it).
    async fn retarget_base(&self, repo: &Path, number: i64, base: &str) -> Result<(), ForgeError>;
    /// Edits the title, and the body unless `None`.
    async fn edit_pr(
        &self,
        repo: &Path,
        number: i64,
        title: &str,
        body: Option<&str>,
    ) -> Result<(), ForgeError>;
    async fn set_draft(&self, repo: &Path, number: i64, draft: bool) -> Result<(), ForgeError>;
    /// Whether `branch` of `owner_repo` requires a PR. `None` is unknown.
    async fn branch_requires_pr(&self, repo: &Path, owner_repo: &str, branch: &str)
    -> Option<bool>;

    // ---- Conversation
    /// A conversation comment, or a line note when `file` and `line` are set.
    async fn post_comment(
        &self,
        repo: &Path,
        number: i64,
        file: &str,
        line: i64,
        body: &str,
    ) -> Result<(), ForgeError>;
    /// Submits a review (`approve`, `request`, or a comment) with pending
    /// line notes.
    async fn submit_review(
        &self,
        repo: &Path,
        number: i64,
        kind: &str,
        body: &str,
        comments: &[PendingLineComment],
    ) -> Result<(), ForgeError>;
    async fn edit_comment(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
    ) -> Result<(), ForgeError>;
    async fn delete_comment(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
    ) -> Result<(), ForgeError>;
    /// Replies on a line-note thread; the created row when the forge echoed it.
    async fn reply(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
        pr_url: &str,
    ) -> Result<Option<PrComment>, ForgeError>;
    async fn set_thread_resolved(
        &self,
        repo: &Path,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), ForgeError>;
    /// The node a reaction on this row targets; `None` when the row cannot
    /// take reactions.
    fn reaction_subject(&self, comment: &PrComment, pr: &Pr) -> Option<String>;
    async fn set_reaction(
        &self,
        repo: &Path,
        subject: &str,
        content: &str,
        add: bool,
    ) -> Result<(), ForgeError>;
    /// People who can be @mentioned on the PR.
    async fn mentionable_users(&self, repo: &Path, pr_url: &str, query: &str)
    -> Vec<PrParticipant>;

    // ---- Merge
    /// Merges, or enqueues when the base uses a merge queue.
    async fn merge(
        &self,
        repo: &Path,
        pr: &Pr,
        method: &MergeMethod,
    ) -> Result<MergeOutcome, ForgeError>;
    /// User-facing (HTML) note for a failed merge.
    fn merge_failure_note(&self, err: &ForgeError) -> String;
    /// Merge commit text the forge would use.
    fn merge_message(
        &self,
        pr: &Pr,
        method: &MergeMethod,
        squash: &SquashCommitMessage,
        merge: &MergeCommitMessage,
    ) -> MergeCommitText;
    /// A PR body capped to what the forge stores.
    fn format_pr_body(&self, body: &str) -> String;
    /// (title max, title max on retry after a too-long rejection), in chars.
    fn title_limits(&self) -> (usize, usize);
    /// Command output only, from a [`Forge::create_pr`] failure reason.
    fn failure_diag(&self, reason: &str) -> String;

    // ---- Images and agent-facing text
    /// Authenticated fetch of a forge-hosted image; `Ok(None)` when the URL
    /// is not this forge's.
    async fn fetch_image(&self, url: &str) -> Result<Option<ImageBytes>, ForgeError>;
    /// How agents should refer to the PR (`GitHub pull request #9 (url)`).
    fn pr_label(&self, s: &PrSubject) -> String;
    /// Turn subject text for a chat looking at a PR of this forge.
    fn pr_subject_prompt(&self, s: &PrSubject) -> String;

    // ---- Web links (PR views)
    fn comment_url(&self, pr_url: &str, comment: &PrComment) -> String;
    fn commit_url(&self, pr_url: &str, sha: &str) -> String;
    fn blob_url(&self, pr_url: &str, git_ref: &str, path: &str, line: i64) -> String;
    fn check_run_url(&self, url: &str, pr_url: &str, pr_number: i64, status: &str) -> String;
    fn checks_url(&self, pr_url: &str) -> String;
}
