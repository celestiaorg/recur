//! GitHub over the `gh` CLI (same commands, GraphQL queries and parsing as
//! the Go engine) plus authenticated attachment-image fetches.

mod branch_rules;
mod checks;
mod create;
mod images;

mod links;
mod list;
mod merge;
mod mutations;
mod reactions;
mod repo;
mod snapshot;
#[cfg(test)]
mod tests;
mod thread;
mod timeline;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use recur_domain::{
    MergeCommitMessage, MergeMethod, PendingLineComment, Pr, PrComment, PrParticipant, PrSource,
    SquashCommitMessage,
};

pub use checks::{CHECKS_QUERY, checks_tab_url};
pub use images::{ImageResponse, ImageTransport, allowed_image_redirect, allowed_image_url};
pub use links::{
    blob_url, check_run_url, comment_permalink, commit_url, node_database_id as ui_node_database_id,
};
pub use merge::{MergeFields, overlay_merge_fields, parse_merge_fields, without_merge_fields};
pub use reactions::ReactionHit;
pub use repo::{
    fork_remote_url, owner_repo_from_pr_url, owner_repo_from_remote_url, pr_number_from_url,
    split_owner_repo,
};
pub use snapshot::{SNAPSHOT_QUERY, snapshot_query};
pub use thread::{ReviewThread, node_database_id};

use crate::cli::Gh;
use crate::model::hard_truncate_chars;
use crate::{
    ChecksReport, CreatePr, Forge, ForgeError, HeadRef, ImageBytes, MergeCommitText, MergeOutcome,
    PrListRow, PrListState, PrSnapshot, PrStateProbe, PrSubject, Viewer,
};

/// How long a `gh auth status` probe is reused.
const AVAILABILITY_TTL: Duration = Duration::from_secs(30);
/// GitHub's PR body limit is 65536 characters; stay well under it.
const BODY_MAX: usize = 60000;

/// Avatar URL for a GitHub login: `provided` when set, else
/// `https://github.com/<login>.png?size=<size>` for a plain login (bots
/// without `[bot]`).
pub fn avatar_url(login: &str, provided: &str, size: u32) -> String {
    let provided = provided.trim();
    if !provided.is_empty() {
        return provided.to_owned();
    }
    let login = login.trim();
    let login = login.strip_suffix("[bot]").unwrap_or(login);
    if login.is_empty() || login.eq_ignore_ascii_case("someone") {
        return String::new();
    }
    if !login
        .bytes()
        .all(|b| b == b'-' || b == b'_' || b.is_ascii_alphanumeric())
    {
        return String::new();
    }
    format!("https://github.com/{login}.png?size={size}")
}

/// Display name for a GitHub login: `Copilot` for its bots, `[bot]` dropped.
pub fn display_name(login: &str) -> String {
    let login = login.trim();
    if login.to_lowercase().contains("copilot") {
        return "Copilot".to_owned();
    }
    login.strip_suffix("[bot]").unwrap_or(login).to_owned()
}

/// `pull/N/head`, kept locally at `refs/recur/pr/N`.
pub fn head_ref(number: i64) -> HeadRef {
    HeadRef {
        remote_ref: format!("pull/{number}/head"),
        local_ref: format!("refs/recur/pr/{number}"),
    }
}

/// The GitHub forge.
pub struct GitHub {
    gh: Gh,
    availability: Mutex<Option<(Instant, bool)>>,
    owner_cache: Mutex<HashMap<PathBuf, String>>,
    checks_logged: Mutex<HashSet<String>>,
    merge_retry_delay: Duration,
    image_token: Mutex<Option<(String, Instant)>>,
    image_transport: Arc<dyn ImageTransport>,
}

impl Default for GitHub {
    fn default() -> Self {
        Self::new()
    }
}

impl GitHub {
    /// GitHub through `gh` on `PATH`.
    pub fn new() -> Self {
        Self::with_gh(Gh::default())
    }

    /// GitHub through a specific gh executable.
    pub fn with_gh(gh: Gh) -> Self {
        Self {
            gh,
            availability: Mutex::new(None),
            owner_cache: Mutex::default(),
            checks_logged: Mutex::default(),
            merge_retry_delay: Duration::from_secs(1),
            image_token: Mutex::new(None),
            image_transport: Arc::new(images::HttpTransport::new()),
        }
    }

    /// Pause between REST merge attempts while mergeability is computed.
    pub fn with_merge_retry_delay(mut self, delay: Duration) -> Self {
        self.merge_retry_delay = delay;
        self
    }

    pub fn with_image_transport(mut self, transport: Arc<dyn ImageTransport>) -> Self {
        self.image_transport = transport;
        self
    }

    pub fn gh(&self) -> &Gh {
        &self.gh
    }

    /// Go `ghCLIAvailable`: `gh` installed and `gh auth status` passes,
    /// reused for 30 s. Conservative: a hung or missing gh is unavailable.
    pub async fn cli_available(&self) -> bool {
        if let Some((at, ok)) = *self.availability.lock()
            && at.elapsed() < AVAILABILITY_TTL
        {
            return ok;
        }
        let ok = self.probe().await;
        *self.availability.lock() = Some((Instant::now(), ok));
        ok
    }

    /// Drops the cached availability probe.
    pub fn reset_availability(&self) {
        *self.availability.lock() = None;
    }

    async fn probe(&self) -> bool {
        if !self.gh.installed() {
            return false;
        }
        let run = self.gh.output(Path::new(""), &["auth", "status"]);
        matches!(tokio::time::timeout(Duration::from_secs(5), run).await, Ok(o) if o.ok())
    }

    /// Go `GitHubLogin` / `fetchGitHubLogin`: `gh api user -q .login`, or
    /// empty.
    pub async fn viewer_login(&self) -> String {
        if !self.gh.installed() {
            return String::new();
        }
        match self
            .gh
            .run(Path::new(""), &["api", "user", "-q", ".login"])
            .await
        {
            Ok(out) => out.trim().to_owned(),
            Err(_) => String::new(),
        }
    }

    /// Go `fetchGitHubReviewThreads` (live comparisons; snapshots read
    /// threads from the combined query).
    pub async fn review_threads(
        &self,
        repo: &Path,
        owner_repo: &str,
        number: i64,
    ) -> Result<Vec<ReviewThread>, ForgeError> {
        let Some((owner, name)) = split_owner_repo(owner_repo).filter(|_| number > 0) else {
            return Err(ForgeError::invalid("invalid repo for review threads"));
        };
        const Q: &str = "query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){reviewThreads(first:100){nodes{id isResolved comments(first:50){nodes{databaseId}}}}}}}";
        let out = self
            .gh
            .run(repo, &graphql_args(Q, &owner, &name, number))
            .await?;
        Ok(thread::parse_review_threads(&out))
    }

    /// Go `fetchGitHubCommitRollup`: commit signatures/checks and actor
    /// avatars (live comparisons; snapshots read them from the combined query).
    pub async fn commit_rollup(
        &self,
        repo: &Path,
        owner_repo: &str,
        number: i64,
    ) -> (HashMap<String, crate::CommitMeta>, HashMap<String, String>) {
        let Some((owner, name)) = split_owner_repo(owner_repo).filter(|_| number > 0) else {
            return Default::default();
        };
        const Q: &str = "query($o:String!,$r:String!,$n:Int!){repository(owner:$o,name:$r){pullRequest(number:$n){author{login avatarUrl}comments(last:100){nodes{author{login avatarUrl}}}reviews(last:100){nodes{author{login avatarUrl}}}commits(last:100){nodes{commit{oid url signature{isValid} statusCheckRollup{state}}}}}}}";
        match self
            .gh
            .run(repo, &graphql_args(Q, &owner, &name, number))
            .await
        {
            Ok(out) => thread::parse_commit_rollup(&out),
            Err(_) => Default::default(),
        }
    }
}

/// `gh api graphql -f query=… -F o=… -F r=… -F n=…`.
fn graphql_args(query: &str, owner: &str, name: &str, number: i64) -> Vec<String> {
    vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!("query={query}"),
        "-F".into(),
        format!("o={owner}"),
        "-F".into(),
        format!("r={name}"),
        "-F".into(),
        format!("n={number}"),
    ]
}

#[async_trait]
impl Forge for GitHub {
    fn id(&self) -> &str {
        "github"
    }

    fn pr_source(&self) -> PrSource {
        PrSource::GitHub
    }

    fn label(&self) -> &str {
        "GitHub"
    }

    fn detect(&self, remote_url: &str) -> Option<String> {
        owner_repo_from_remote_url(remote_url)
    }

    fn imported_pr_id(&self, project_id: &str, number: i64) -> String {
        format!("gh-{}-{number}", project_id.trim())
    }

    fn avatar_url(&self, login: &str, provided: &str, size: u32) -> String {
        avatar_url(login, provided, size)
    }

    async fn available(&self) -> bool {
        self.cli_available().await
    }

    async fn login(&self) -> String {
        self.viewer_login().await
    }

    async fn viewer(&self, repo: &Path, owner_repo: &str) -> Viewer {
        self.fetch_viewer(repo, owner_repo).await
    }

    async fn remote_repo(&self, repo: &Path, remote: &str) -> Result<String, ForgeError> {
        self.owner_repo_for_remote(repo, remote).await
    }

    async fn list_prs(
        &self,
        repo: &Path,
        state: PrListState,
        limit: usize,
    ) -> Result<Vec<PrListRow>, ForgeError> {
        self.list(repo, state, limit).await
    }

    async fn pr_state(&self, repo: &Path, number: i64) -> Result<PrStateProbe, ForgeError> {
        self.view_state(repo, number).await
    }

    async fn snapshot(&self, repo: &Path, number: i64) -> Result<PrSnapshot, ForgeError> {
        self.fetch_snapshot(repo, number).await
    }

    async fn diff(&self, repo: &Path, number: i64) -> Result<String, ForgeError> {
        Ok(self
            .gh
            .run(
                repo,
                &["pr".to_owned(), "diff".to_owned(), number.to_string()],
            )
            .await?)
    }

    async fn checks(
        &self,
        repo: &Path,
        number: i64,
        pr_url: &str,
    ) -> Result<ChecksReport, ForgeError> {
        self.fetch_checks(repo, number, pr_url).await
    }

    fn merge_comments(
        &self,
        existing: &[PrComment],
        snap: &PrSnapshot,
        now: DateTime<Utc>,
    ) -> Vec<PrComment> {
        thread::merge_comments(
            existing,
            &snap.comments,
            snap.comments_truncated,
            snap.reviews_truncated,
            now,
        )
    }

    async fn create_pr(&self, req: CreatePr<'_>) -> Result<i64, String> {
        self.create(req).await
    }

    async fn push_head(
        &self,
        worktree: &Path,
        repo: &Path,
        pr: &Pr,
        remote_head: &str,
        force: bool,
    ) -> Result<(), ForgeError> {
        self.push(worktree, repo, pr, remote_head, force).await
    }

    fn head_ref(&self, number: i64) -> HeadRef {
        head_ref(number)
    }

    async fn retarget_base(&self, repo: &Path, number: i64, base: &str) -> Result<(), ForgeError> {
        if !self.cli_available().await {
            return Err(ForgeError::unavailable(
                "gh CLI is not installed or not authenticated",
            ));
        }
        self.gh
            .run(
                repo,
                &[
                    "pr".to_owned(),
                    "edit".to_owned(),
                    number.to_string(),
                    "--base".to_owned(),
                    base.to_owned(),
                ],
            )
            .await?;
        Ok(())
    }

    async fn edit_pr(
        &self,
        repo: &Path,
        number: i64,
        title: &str,
        body: Option<&str>,
    ) -> Result<(), ForgeError> {
        self.edit(repo, number, title, body).await
    }

    async fn set_draft(&self, repo: &Path, number: i64, draft: bool) -> Result<(), ForgeError> {
        self.draft(repo, number, draft).await
    }

    async fn branch_requires_pr(
        &self,
        repo: &Path,
        owner_repo: &str,
        branch: &str,
    ) -> Option<bool> {
        self.fetch_branch_requires_pr(repo, owner_repo, branch)
            .await
    }

    async fn post_comment(
        &self,
        repo: &Path,
        number: i64,
        file: &str,
        line: i64,
        body: &str,
    ) -> Result<(), ForgeError> {
        self.post(repo, number, file, line, body).await
    }

    async fn submit_review(
        &self,
        repo: &Path,
        number: i64,
        kind: &str,
        body: &str,
        comments: &[PendingLineComment],
    ) -> Result<(), ForgeError> {
        self.review(repo, number, kind, body, comments).await
    }

    async fn edit_comment(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        self.edit_comment_body(repo, number, comment_id, body).await
    }

    async fn delete_comment(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
    ) -> Result<(), ForgeError> {
        self.delete(repo, number, comment_id).await
    }

    async fn reply(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
        pr_url: &str,
    ) -> Result<Option<PrComment>, ForgeError> {
        self.reply_to(repo, number, comment_id, body, pr_url).await
    }

    async fn set_thread_resolved(
        &self,
        repo: &Path,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), ForgeError> {
        self.resolve_thread(repo, thread_id, resolved).await
    }

    fn reaction_subject(&self, comment: &PrComment, pr: &Pr) -> Option<String> {
        reactions::reaction_subject_id(comment, pr)
    }

    async fn set_reaction(
        &self,
        repo: &Path,
        subject: &str,
        content: &str,
        add: bool,
    ) -> Result<(), ForgeError> {
        self.set_reaction_on(repo, subject, content, add).await
    }

    async fn mentionable_users(
        &self,
        repo: &Path,
        pr_url: &str,
        query: &str,
    ) -> Vec<PrParticipant> {
        self.search_mentionable(repo, pr_url, query).await
    }

    async fn merge(
        &self,
        repo: &Path,
        pr: &Pr,
        method: &MergeMethod,
    ) -> Result<MergeOutcome, ForgeError> {
        self.merge_pr(repo, pr, method).await
    }

    fn merge_failure_note(&self, err: &ForgeError) -> String {
        merge::merge_failure_note(&err.message)
    }

    fn merge_message(
        &self,
        pr: &Pr,
        method: &MergeMethod,
        squash: &SquashCommitMessage,
        merge: &MergeCommitMessage,
    ) -> MergeCommitText {
        merge::merge_message(pr, method, squash, merge)
    }

    fn format_pr_body(&self, body: &str) -> String {
        if body.chars().count() <= BODY_MAX {
            return body.to_owned();
        }
        format!("{}\n\n…(truncated)…", hard_truncate_chars(body, BODY_MAX))
    }

    fn title_limits(&self) -> (usize, usize) {
        (create::TITLE_MAX, create::TITLE_RETRY_MAX)
    }

    fn failure_diag(&self, reason: &str) -> String {
        create::fail_diag(reason)
    }

    async fn fetch_image(&self, url: &str) -> Result<Option<ImageBytes>, ForgeError> {
        self.image(url).await
    }

    fn pr_label(&self, s: &PrSubject) -> String {
        let url = s.url.trim();
        match (s.number > 0, url.is_empty()) {
            (true, false) => format!("GitHub pull request #{} ({url})", s.number),
            (true, true) => format!("GitHub pull request #{}", s.number),
            (false, false) => format!("GitHub pull request ({url})"),
            (false, true) => "this GitHub pull request".to_owned(),
        }
    }

    fn pr_subject_prompt(&self, s: &PrSubject) -> String {
        let mut b = format!("The operator is looking at {}", self.pr_label(s));
        let title = s.title.trim();
        if !title.is_empty() {
            b.push_str(&format!(" titled \"{title}\""));
        }
        let head = s.branch.trim();
        let base = s.base.trim();
        if !head.is_empty() || !base.is_empty() {
            b.push_str(" (");
            if !head.is_empty() {
                b.push_str("head ");
                b.push_str(head);
            }
            if !head.is_empty() && !base.is_empty() {
                b.push_str(" into ");
            }
            b.push_str(base);
            b.push(')');
        }
        b.push_str(". Questions, plans, and reviews are about this pull request — use `gh pr view`, `gh pr diff`, and existing review threads. Do not treat `git diff` in the current directory as the PR unless cwd is actually checked out on the PR head.");
        if s.host {
            b.push_str(" This chat is attached to the PR, not a Recur worktree of its head; cwd is the project's working copy and may be a different branch.");
        } else {
            b.push_str(" This chat runs in the Recur workspace for that PR's head; uncommitted local work is not on GitHub until pushed.");
        }
        b
    }

    fn comment_url(&self, pr_url: &str, comment: &PrComment) -> String {
        comment_permalink(pr_url, comment)
    }

    fn commit_url(&self, pr_url: &str, sha: &str) -> String {
        commit_url(pr_url, sha)
    }

    fn blob_url(&self, pr_url: &str, git_ref: &str, path: &str, line: i64) -> String {
        blob_url(pr_url, git_ref, path, line)
    }

    fn check_run_url(&self, url: &str, pr_url: &str, pr_number: i64, status: &str) -> String {
        check_run_url(url, pr_url, pr_number, status)
    }

    fn checks_url(&self, pr_url: &str) -> String {
        checks_tab_url(pr_url)
    }
}
