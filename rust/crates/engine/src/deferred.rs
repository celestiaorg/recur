//! Deferred call sites (plan §5, rule 1): E1 functions whose Go version
//! calls into code a later engine slice owns. Each hook is the exact Go call
//! site, named after the Go function it stands for; its body is what that
//! call does while the subsystem it reaches does not exist yet (no pending
//! questions, no remounts, no PR chat hosts, …). The owning slice replaces
//! the body with the real call when its module lands and deletes the hook's
//! entry from the plan's deferred list.

use recur_domain::{Check, Pr, PrSource, Project, Workspace};

use crate::Engine;

impl Engine {
    // ---- E5: pull requests

    /// Go `resolveHoistedChat`: a send that still names a PR chat host whose
    /// chats were hoisted onto a worktree follows them (`chatHoistAlias`).
    /// E5 (`hoist`).
    pub(crate) fn resolve_hoisted_chat(&self, workspace_id: &str, code: &str) -> (String, String) {
        (workspace_id.to_owned(), code.to_owned())
    }

    /// Go `maybeArchiveLinkedPRChatHost`: archive a PR chat host once its
    /// chats moved to a real worktree. E5 (`hoist`).
    pub(crate) fn maybe_archive_linked_pr_chat_host(&self, _workspace_id: &str) {}

    /// Go `linkWorkspaceToMatchingPRs`: attach imported PRs whose head is
    /// this workspace's branch. E5 (`prs::linking`).
    pub(crate) async fn link_workspace_to_matching_prs(&self, _ws: &Workspace) {}

    /// Go `archivePRChatHosts`: hoist a PR's chat hosts onto the workspace
    /// that now checks it out. E5 (`hoist`).
    pub(crate) async fn archive_pr_chat_hosts(&self, _pr_id: &str, _keep_id: &str) {}

    /// Go `fetchOpenPRRemote`: refresh an open forge PR's head ref. E5.
    pub(crate) async fn fetch_open_pr_remote(&self, _ws: &Workspace) {}

    /// Go `CreateGitHubPR` (+ `githubPRBase`, `githubImportedPRID`): push the
    /// branch and open the PR on the project's forge for a project that
    /// opts in. `None` while no forge PR glue exists, so the walk stays on a
    /// Local PR; `Err((forge label, reason))` is the fallback note. E5
    /// (`prs::ops`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_forge_pr(
        &self,
        _proj: &Project,
        _worktree: &str,
        _branch: &str,
        _base: &str,
        _title: &str,
        _body: &str,
        _draft: bool,
    ) -> Option<Result<ForgePrCreated, (String, String)>> {
        None
    }

    /// Go `pushGitHubHead`: push a rework onto an open forge PR's remote
    /// head. `Err((forge label, reason))`. Only reached for a forge PR, which
    /// E5 creates. E5 (`prs::ops`).
    pub(crate) async fn push_forge_pr_head(
        &self,
        _worktree: &str,
        _repo_path: &str,
        _pr: &Pr,
        _head: &str,
        _force: bool,
    ) -> Result<(), (String, String)> {
        Ok(())
    }

    /// Go `githubPRChecks`: the forge's check rollup at PR open; a fetch
    /// error is an empty rollup that counts as passing (unknown is not a
    /// failure), which is also the answer with no forge glue. E5.
    pub(crate) async fn forge_pr_checks(
        &self,
        _source: &PrSource,
        _repo_path: &str,
        _number: i64,
    ) -> (Vec<Check>, bool) {
        (Vec::new(), true)
    }
}

/// A PR the forge opened (`create_forge_pr`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForgePrCreated {
    pub source: PrSource,
    pub number: i64,
    pub pr_id: String,
}
