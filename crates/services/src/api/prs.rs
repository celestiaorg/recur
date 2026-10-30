use async_trait::async_trait;
use recur_domain::{
    Commit, FileDiff, Finding, Pr, PrComment, PrParticipant, UpdateWorkspaceResult, Workspace,
};

use crate::Result;

#[async_trait]
pub trait PrsService: Send + Sync {
    /// AddPRComment records a review note. A file+line note stays pending until
    /// SubmitReview (or SendBackToLoop). Conversation-only notes (no file) still
    /// post immediately. startLine is the first line of a multiline comment (0 for
    /// a single line). findingID tags a line draft queued from an observatory
    /// finding. Returns the persisted comment (or a stub after a pending add).
    async fn add_pr_comment(
        &self,
        pr_id: &str,
        file: &str,
        line: i64,
        body: &str,
        start_line: i64,
        finding_id: &str,
    ) -> Result<PrComment>;

    /// CreateWorkspaceFromPR creates a Recur workspace on this PR's head. Import
    /// already attaches a workspace that is already on the PR's GitHub head branch.
    async fn create_workspace_from_pr(&self, pr_id: &str) -> Result<Workspace>;

    /// DeletePRComment removes a line note or standalone conversation comment the
    /// viewer is allowed to delete. Submitted reviews cannot be deleted.
    async fn delete_pr_comment(&self, pr_id: &str, comment_id: &str) -> Result<Pr>;

    /// EnsurePRChatHost returns a workspace the PR-detail agent dock can bind to
    /// when the PR has no Recur worktree yet. CreateWorkspaceFromPR promotes that
    /// same row in place so chat history survives.
    async fn ensure_pr_chat_host(&self, pr_id: &str) -> Result<Workspace>;

    async fn get_commit_diff(&self, id: &str, hash: &str) -> Result<Commit>;

    async fn get_pr(&self, id: &str) -> Result<Pr>;

    async fn get_pr_commits(&self, id: &str) -> Result<Vec<Commit>>;

    async fn get_pr_files(&self, id: &str) -> Result<Vec<FileDiff>>;

    /// ListPRFindings returns reviewer findings for the PR: the linked workspace
    /// when one exists, otherwise the hidden PR chat host (imported GitHub PRs).
    async fn list_pr_findings(&self, pr_id: &str) -> Result<Vec<Finding>>;

    async fn list_prs(&self) -> Result<Vec<Pr>>;

    /// MergePR merges the PR (PRD §12.1, D53). Local = merge the branch into base
    /// ON DISK; the user is the sole gate. method is the merge strategy
    /// ("merge"/"squash"/"rebase"); empty = resolve from project/settings prefs.
    async fn merge_pr(&self, pr_id: &str, method: &str) -> Result<Pr>;

    /// ReadyPR marks a GitHub draft PR ready for review (`gh pr ready`).
    async fn ready_pr(&self, pr_id: &str) -> Result<Pr>;

    /// ReplyPRComment posts a reply on a line-note thread. Conversation comments
    /// cannot be replied to in-thread.
    async fn reply_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<PrComment>;

    async fn search_pr_mentionable_users(
        &self,
        pr_id: &str,
        query: &str,
    ) -> Result<Vec<PrParticipant>>;

    /// SendBackToLoop is the PR Send Back control: a conductor turn with the review
    /// thread attached. Maestro answers or calls reenter_phase.
    async fn send_back_to_loop(&self, pr_id: &str, comments: &[String]) -> Result<Pr>;

    /// SetPRDraft converts a PR to a draft or marks it ready for review.
    /// GitHub: `gh pr ready` / `gh pr ready --undo`. Local: persist the flag.
    async fn set_pr_draft(&self, pr_id: &str, draft: bool) -> Result<Pr>;

    /// SetPRThreadResolved marks a line-note conversation resolved or reopens it.
    /// GitHub: PR author or write access. Local: always allowed.
    async fn set_pr_thread_resolved(
        &self,
        pr_id: &str,
        thread_id: &str,
        resolved: bool,
    ) -> Result<Pr>;

    /// SubmitReview persists a review (Comment/Approve/Request changes) as a thread entry
    /// and emits pr.comment/pr.updated (PRD §12.2, §14). Approve/Request-changes also
    /// reflect on the PR state. Returns the updated PR.
    async fn submit_review(&self, pr_id: &str, kind: &str, body: &str) -> Result<Pr>;

    /// SyncGitHubPRs lists GitHub pull requests for every Recur git project and
    /// merges them into the dashboard (including PRs Recur did not open).
    async fn sync_github_prs(&self) -> Result<Vec<Pr>>;

    /// TogglePRCommentReaction adds or removes the viewer's GitHub reaction on a
    /// conversation item (description, comment, review, or line note).
    async fn toggle_pr_comment_reaction(
        &self,
        pr_id: &str,
        comment_id: &str,
        content: &str,
    ) -> Result<Pr>;

    /// UpdatePRComment edits a conversation comment the viewer owns.
    async fn update_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<PrComment>;

    /// UpdatePRDetails edits the title and description. GitHub PRs require the
    /// viewer to be the author or have write access.
    async fn update_pr_details(&self, pr_id: &str, title: &str, body: &str) -> Result<Pr>;

    /// UpdateWithBase integrates the latest base tip into the PR worktree (D54).
    /// Clean path is a direct git merge/rebase; conflicts leave markers and return
    /// Status=conflict so the UI invokes /update. method is "merge" or "rebase";
    /// empty uses project/settings UpdateBaseMethod prefs.
    async fn update_with_base(&self, pr_id: &str, method: &str) -> Result<UpdateWorkspaceResult>;
}
