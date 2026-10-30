//! [`FakeForge`]: a GitHub-shaped [`Forge`] for engine and services tests.
//!
//! Every I/O method records a [`FakeCall`] and runs the handler installed
//! with the matching `on_*` setter, or a harmless default (Ok/empty;
//! `snapshot` and `checks` fail like an unreachable gh). Pure methods
//! (ids, messages, links, comment merging) use the real GitHub rules.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use parking_lot::Mutex;
use recur_domain::{
    MergeCommitMessage, MergeMethod, PendingLineComment, Pr, PrComment, PrParticipant, PrSource,
    SquashCommitMessage,
};

use crate::github::GitHub;
use crate::{
    ChecksReport, CreatePr, Forge, ForgeError, HeadRef, ImageBytes, MergeCommitText, MergeOutcome,
    PrListRow, PrListState, PrSnapshot, PrStateProbe, PrSubject, Viewer,
};

type Handler<A, R> = Arc<dyn Fn(A) -> BoxFuture<'static, R> + Send + Sync>;

/// One recorded I/O call: method name and its arguments as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeCall {
    pub method: &'static str,
    pub args: Vec<String>,
}

macro_rules! handlers {
    ($($setter:ident: ($($arg:ty),*) -> $ret:ty;)*) => {
        #[derive(Default)]
        struct Handlers {
            $($setter: Option<Handler<($($arg,)*), $ret>>,)*
        }

        impl FakeForge {
            $(
                pub fn $setter<F, Fut>(&self, f: F) -> &Self
                where
                    F: Fn(($($arg,)*)) -> Fut + Send + Sync + 'static,
                    Fut: Future<Output = $ret> + Send + 'static,
                {
                    self.handlers.lock().$setter = Some(Arc::new(move |a| Box::pin(f(a))));
                    self
                }
            )*
        }
    };
}

handlers! {
    on_available: () -> bool;
    on_login: () -> String;
    on_viewer: (PathBuf, String) -> Viewer;
    on_remote_repo: (PathBuf, String) -> Result<String, ForgeError>;
    on_list_prs: (PathBuf, PrListState, usize) -> Result<Vec<PrListRow>, ForgeError>;
    on_pr_state: (PathBuf, i64) -> Result<PrStateProbe, ForgeError>;
    on_snapshot: (PathBuf, i64) -> Result<PrSnapshot, ForgeError>;
    on_diff: (PathBuf, i64) -> Result<String, ForgeError>;
    on_checks: (PathBuf, i64, String) -> Result<ChecksReport, ForgeError>;
    on_create_pr: (FakeCreate) -> Result<i64, String>;
    on_push_head: (PathBuf, PathBuf, Pr, String, bool) -> Result<(), ForgeError>;
    on_retarget_base: (PathBuf, i64, String) -> Result<(), ForgeError>;
    on_edit_pr: (PathBuf, i64, String, Option<String>) -> Result<(), ForgeError>;
    on_set_draft: (PathBuf, i64, bool) -> Result<(), ForgeError>;
    on_branch_requires_pr: (PathBuf, String, String) -> Option<bool>;
    on_post_comment: (PathBuf, i64, String, i64, String) -> Result<(), ForgeError>;
    on_submit_review: (PathBuf, i64, String, String, Vec<PendingLineComment>) -> Result<(), ForgeError>;
    on_edit_comment: (PathBuf, i64, String, String) -> Result<(), ForgeError>;
    on_delete_comment: (PathBuf, i64, String) -> Result<(), ForgeError>;
    on_reply: (PathBuf, i64, String, String, String) -> Result<Option<PrComment>, ForgeError>;
    on_set_thread_resolved: (PathBuf, String, bool) -> Result<(), ForgeError>;
    on_set_reaction: (PathBuf, String, String, bool) -> Result<(), ForgeError>;
    on_mentionable_users: (PathBuf, String, String) -> Vec<PrParticipant>;
    on_merge: (PathBuf, Pr, MergeMethod) -> Result<MergeOutcome, ForgeError>;
    on_fetch_image: (String) -> Result<Option<ImageBytes>, ForgeError>;
}

/// The arguments of a [`Forge::create_pr`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeCreate {
    pub repo: PathBuf,
    pub worktree: PathBuf,
    pub branch: String,
    pub base: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

pub struct FakeForge {
    rules: GitHub,
    handlers: Mutex<Handlers>,
    calls: Mutex<Vec<FakeCall>>,
}

impl Default for FakeForge {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeForge {
    pub fn new() -> Self {
        Self {
            rules: GitHub::with_gh(crate::Gh::new("/nonexistent/recur-fake-gh")),
            handlers: Mutex::default(),
            calls: Mutex::default(),
        }
    }

    /// Every recorded call, in order.
    pub fn calls(&self) -> Vec<FakeCall> {
        self.calls.lock().clone()
    }

    /// Arguments of each call to `method`, in order.
    pub fn calls_to(&self, method: &str) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .iter()
            .filter(|c| c.method == method)
            .map(|c| c.args.clone())
            .collect()
    }

    pub fn clear_calls(&self) {
        self.calls.lock().clear();
    }

    fn record(&self, method: &'static str, args: Vec<String>) {
        self.calls.lock().push(FakeCall { method, args });
    }
}

fn p(path: &Path) -> String {
    path.display().to_string()
}

macro_rules! dispatch {
    ($self:ident, $setter:ident, $args:expr, $default:expr) => {{
        let h = $self.handlers.lock().$setter.clone();
        match h {
            Some(h) => h($args).await,
            None => $default,
        }
    }};
}

#[async_trait]
impl Forge for FakeForge {
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
        self.rules.detect(remote_url)
    }

    fn imported_pr_id(&self, project_id: &str, number: i64) -> String {
        self.rules.imported_pr_id(project_id, number)
    }

    fn avatar_url(&self, login: &str, provided: &str, size: u32) -> String {
        self.rules.avatar_url(login, provided, size)
    }

    async fn available(&self) -> bool {
        self.record("available", Vec::new());
        dispatch!(self, on_available, (), true)
    }

    async fn login(&self) -> String {
        self.record("login", Vec::new());
        dispatch!(self, on_login, (), String::new())
    }

    async fn viewer(&self, repo: &Path, owner_repo: &str) -> Viewer {
        self.record("viewer", vec![p(repo), owner_repo.into()]);
        dispatch!(
            self,
            on_viewer,
            (repo.into(), owner_repo.into()),
            Viewer::default()
        )
    }

    async fn remote_repo(&self, repo: &Path, remote: &str) -> Result<String, ForgeError> {
        self.record("remote_repo", vec![p(repo), remote.into()]);
        dispatch!(
            self,
            on_remote_repo,
            (repo.into(), remote.into()),
            Err(ForgeError::failed(format!(
                "remote {remote} is not a GitHub URL"
            )))
        )
    }

    async fn list_prs(
        &self,
        repo: &Path,
        state: PrListState,
        limit: usize,
    ) -> Result<Vec<PrListRow>, ForgeError> {
        self.record(
            "list_prs",
            vec![p(repo), state.as_str().into(), limit.to_string()],
        );
        dispatch!(
            self,
            on_list_prs,
            (repo.into(), state, limit),
            Ok(Vec::new())
        )
    }

    async fn pr_state(&self, repo: &Path, number: i64) -> Result<PrStateProbe, ForgeError> {
        self.record("pr_state", vec![p(repo), number.to_string()]);
        dispatch!(
            self,
            on_pr_state,
            (repo.into(), number),
            Ok(PrStateProbe::default())
        )
    }

    async fn snapshot(&self, repo: &Path, number: i64) -> Result<PrSnapshot, ForgeError> {
        self.record("snapshot", vec![p(repo), number.to_string()]);
        dispatch!(
            self,
            on_snapshot,
            (repo.into(), number),
            Err(ForgeError::unavailable("FakeForge: no snapshot handler"))
        )
    }

    async fn diff(&self, repo: &Path, number: i64) -> Result<String, ForgeError> {
        self.record("diff", vec![p(repo), number.to_string()]);
        dispatch!(self, on_diff, (repo.into(), number), Ok(String::new()))
    }

    async fn checks(
        &self,
        repo: &Path,
        number: i64,
        pr_url: &str,
    ) -> Result<ChecksReport, ForgeError> {
        self.record("checks", vec![p(repo), number.to_string(), pr_url.into()]);
        dispatch!(
            self,
            on_checks,
            (repo.into(), number, pr_url.into()),
            Err(ForgeError::unavailable(
                "gh CLI is not installed or not authenticated"
            ))
        )
    }

    fn merge_comments(
        &self,
        existing: &[PrComment],
        snap: &PrSnapshot,
        now: DateTime<Utc>,
    ) -> Vec<PrComment> {
        self.rules.merge_comments(existing, snap, now)
    }

    async fn create_pr(&self, req: CreatePr<'_>) -> Result<i64, String> {
        let c = FakeCreate {
            repo: req.repo.into(),
            worktree: req.worktree.into(),
            branch: req.branch.into(),
            base: req.base.into(),
            title: req.title.into(),
            body: req.body.into(),
            draft: req.draft,
        };
        self.record(
            "create_pr",
            vec![
                p(req.repo),
                p(req.worktree),
                c.branch.clone(),
                c.base.clone(),
                c.title.clone(),
                c.body.clone(),
                c.draft.to_string(),
            ],
        );
        dispatch!(
            self,
            on_create_pr,
            (c,),
            Err(
                "gh CLI is not installed or not authenticated: using a Local PR instead".to_owned()
            )
        )
    }

    async fn push_head(
        &self,
        worktree: &Path,
        repo: &Path,
        pr: &Pr,
        remote_head: &str,
        force: bool,
    ) -> Result<(), ForgeError> {
        self.record(
            "push_head",
            vec![
                p(worktree),
                p(repo),
                pr.id.clone(),
                remote_head.into(),
                force.to_string(),
            ],
        );
        dispatch!(
            self,
            on_push_head,
            (
                worktree.into(),
                repo.into(),
                pr.clone(),
                remote_head.into(),
                force
            ),
            Ok(())
        )
    }

    fn head_ref(&self, number: i64) -> HeadRef {
        self.rules.head_ref(number)
    }

    async fn retarget_base(&self, repo: &Path, number: i64, base: &str) -> Result<(), ForgeError> {
        self.record(
            "retarget_base",
            vec![p(repo), number.to_string(), base.into()],
        );
        dispatch!(
            self,
            on_retarget_base,
            (repo.into(), number, base.into()),
            Ok(())
        )
    }

    async fn edit_pr(
        &self,
        repo: &Path,
        number: i64,
        title: &str,
        body: Option<&str>,
    ) -> Result<(), ForgeError> {
        let mut args = vec![p(repo), number.to_string(), title.into()];
        args.extend(body.map(str::to_owned));
        self.record("edit_pr", args);
        dispatch!(
            self,
            on_edit_pr,
            (repo.into(), number, title.into(), body.map(str::to_owned)),
            Ok(())
        )
    }

    async fn set_draft(&self, repo: &Path, number: i64, draft: bool) -> Result<(), ForgeError> {
        self.record(
            "set_draft",
            vec![p(repo), number.to_string(), draft.to_string()],
        );
        dispatch!(self, on_set_draft, (repo.into(), number, draft), Ok(()))
    }

    async fn branch_requires_pr(
        &self,
        repo: &Path,
        owner_repo: &str,
        branch: &str,
    ) -> Option<bool> {
        self.record(
            "branch_requires_pr",
            vec![p(repo), owner_repo.into(), branch.into()],
        );
        dispatch!(
            self,
            on_branch_requires_pr,
            (repo.into(), owner_repo.into(), branch.into()),
            None
        )
    }

    async fn post_comment(
        &self,
        repo: &Path,
        number: i64,
        file: &str,
        line: i64,
        body: &str,
    ) -> Result<(), ForgeError> {
        self.record(
            "post_comment",
            vec![
                p(repo),
                number.to_string(),
                file.into(),
                line.to_string(),
                body.into(),
            ],
        );
        dispatch!(
            self,
            on_post_comment,
            (repo.into(), number, file.into(), line, body.into()),
            Ok(())
        )
    }

    async fn submit_review(
        &self,
        repo: &Path,
        number: i64,
        kind: &str,
        body: &str,
        comments: &[PendingLineComment],
    ) -> Result<(), ForgeError> {
        self.record(
            "submit_review",
            vec![
                p(repo),
                number.to_string(),
                kind.into(),
                body.into(),
                comments.len().to_string(),
            ],
        );
        dispatch!(
            self,
            on_submit_review,
            (
                repo.into(),
                number,
                kind.into(),
                body.into(),
                comments.to_vec()
            ),
            Ok(())
        )
    }

    async fn edit_comment(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
    ) -> Result<(), ForgeError> {
        self.record(
            "edit_comment",
            vec![p(repo), number.to_string(), comment_id.into(), body.into()],
        );
        dispatch!(
            self,
            on_edit_comment,
            (repo.into(), number, comment_id.into(), body.into()),
            Ok(())
        )
    }

    async fn delete_comment(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
    ) -> Result<(), ForgeError> {
        self.record(
            "delete_comment",
            vec![p(repo), number.to_string(), comment_id.into()],
        );
        dispatch!(
            self,
            on_delete_comment,
            (repo.into(), number, comment_id.into()),
            Ok(())
        )
    }

    async fn reply(
        &self,
        repo: &Path,
        number: i64,
        comment_id: &str,
        body: &str,
        pr_url: &str,
    ) -> Result<Option<PrComment>, ForgeError> {
        self.record(
            "reply",
            vec![
                p(repo),
                number.to_string(),
                comment_id.into(),
                body.into(),
                pr_url.into(),
            ],
        );
        dispatch!(
            self,
            on_reply,
            (
                repo.into(),
                number,
                comment_id.into(),
                body.into(),
                pr_url.into()
            ),
            Ok(None)
        )
    }

    async fn set_thread_resolved(
        &self,
        repo: &Path,
        thread_id: &str,
        resolved: bool,
    ) -> Result<(), ForgeError> {
        self.record(
            "set_thread_resolved",
            vec![p(repo), thread_id.into(), resolved.to_string()],
        );
        dispatch!(
            self,
            on_set_thread_resolved,
            (repo.into(), thread_id.into(), resolved),
            Ok(())
        )
    }

    fn reaction_subject(&self, comment: &PrComment, pr: &Pr) -> Option<String> {
        self.rules.reaction_subject(comment, pr)
    }

    async fn set_reaction(
        &self,
        repo: &Path,
        subject: &str,
        content: &str,
        add: bool,
    ) -> Result<(), ForgeError> {
        self.record(
            "set_reaction",
            vec![p(repo), subject.into(), content.into(), add.to_string()],
        );
        dispatch!(
            self,
            on_set_reaction,
            (repo.into(), subject.into(), content.into(), add),
            Ok(())
        )
    }

    async fn mentionable_users(
        &self,
        repo: &Path,
        pr_url: &str,
        query: &str,
    ) -> Vec<PrParticipant> {
        self.record(
            "mentionable_users",
            vec![p(repo), pr_url.into(), query.into()],
        );
        dispatch!(
            self,
            on_mentionable_users,
            (repo.into(), pr_url.into(), query.into()),
            Vec::new()
        )
    }

    async fn merge(
        &self,
        repo: &Path,
        pr: &Pr,
        method: &MergeMethod,
    ) -> Result<MergeOutcome, ForgeError> {
        self.record(
            "merge",
            vec![p(repo), pr.id.clone(), method.as_str().into()],
        );
        dispatch!(
            self,
            on_merge,
            (repo.into(), pr.clone(), method.clone()),
            Ok(MergeOutcome::Merged)
        )
    }

    fn merge_failure_note(&self, err: &ForgeError) -> String {
        self.rules.merge_failure_note(err)
    }

    fn merge_message(
        &self,
        pr: &Pr,
        method: &MergeMethod,
        squash: &SquashCommitMessage,
        merge: &MergeCommitMessage,
    ) -> MergeCommitText {
        self.rules.merge_message(pr, method, squash, merge)
    }

    fn format_pr_body(&self, body: &str) -> String {
        self.rules.format_pr_body(body)
    }

    fn title_limits(&self) -> (usize, usize) {
        self.rules.title_limits()
    }

    fn failure_diag(&self, reason: &str) -> String {
        self.rules.failure_diag(reason)
    }

    async fn fetch_image(&self, url: &str) -> Result<Option<ImageBytes>, ForgeError> {
        self.record("fetch_image", vec![url.into()]);
        dispatch!(self, on_fetch_image, (url.into(),), Ok(None))
    }

    fn pr_label(&self, s: &PrSubject) -> String {
        self.rules.pr_label(s)
    }

    fn pr_subject_prompt(&self, s: &PrSubject) -> String {
        self.rules.pr_subject_prompt(s)
    }

    fn comment_url(&self, pr_url: &str, comment: &PrComment) -> String {
        self.rules.comment_url(pr_url, comment)
    }

    fn commit_url(&self, pr_url: &str, sha: &str) -> String {
        self.rules.commit_url(pr_url, sha)
    }

    fn blob_url(&self, pr_url: &str, git_ref: &str, path: &str, line: i64) -> String {
        self.rules.blob_url(pr_url, git_ref, path, line)
    }

    fn check_run_url(&self, url: &str, pr_url: &str, pr_number: i64, status: &str) -> String {
        self.rules.check_run_url(url, pr_url, pr_number, status)
    }

    fn checks_url(&self, pr_url: &str) -> String {
        self.rules.checks_url(pr_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn handlers_and_calls() {
        let fake = FakeForge::new();
        assert!(fake.available().await);
        assert!(fake.snapshot(Path::new("/r"), 3).await.is_err());
        fake.on_snapshot(|(_, n)| async move {
            Ok(PrSnapshot {
                title: format!("PR {n}"),
                ..Default::default()
            })
        })
        .on_set_reaction(|(_, subject, _, _)| async move {
            if subject.is_empty() {
                Err(ForgeError::invalid("invalid reaction subject"))
            } else {
                Ok(())
            }
        });
        assert_eq!(
            fake.snapshot(Path::new("/r"), 3).await.unwrap().title,
            "PR 3"
        );
        fake.set_reaction(Path::new("/r"), "IC_1", "heart", true)
            .await
            .unwrap();
        assert_eq!(
            fake.calls_to("set_reaction"),
            [vec![
                "/r".to_owned(),
                "IC_1".into(),
                "heart".into(),
                "true".into()
            ]]
        );
        assert_eq!(fake.calls_to("snapshot").len(), 2);
        assert_eq!(fake.imported_pr_id(" p1 ", 7), "gh-p1-7");
        fake.clear_calls();
        assert!(fake.calls().is_empty());
    }
}
