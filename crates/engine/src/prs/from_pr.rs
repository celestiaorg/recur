//! A workspace for a PR: reuse a workspace already on the head, else allocate
//! one on the PR head; the PR-detail dock gets a hidden chat host on the
//! project checkout until then, and that host is promoted in place so its chats
//! survive.

use recur_domain::{CHAT_MODE_REVIEW, Group, Pr, WORKSPACE_SOLO, Workspace};

use crate::Engine;
use crate::codes::PAIR_AGENT_CODE;
use crate::core::{EngineError, Result};
use crate::prs::pr_settled;
use crate::text::truncate;
use crate::workspace::create::{
    PHASE_CREATING, PrHeadCheckoutSpec, STATUS_CREATING, normalize_pr_branch,
};

/// The local branch a PR-head worktree may create. Fork heads and names
/// that collide with the project or PR base (a fork whose head is `main`)
/// are empty, so allocate uses a unique branch instead of moving `main`.
pub fn checkout_pr_head_name(
    head: &str,
    pr_base: &str,
    project_base: &str,
    cross_repo: bool,
) -> String {
    let head = normalize_pr_branch(head);
    if head.is_empty() || cross_repo {
        return String::new();
    }
    for b in [project_base, pr_base] {
        let b = normalize_pr_branch(b);
        if !b.is_empty() && head.eq_ignore_ascii_case(&b) {
            return String::new();
        }
    }
    head
}

fn pr_workspace_name(pr: &Pr) -> String {
    let name = pr.title.trim();
    if name.is_empty() {
        format!("PR #{}", pr.number)
    } else {
        name.to_owned()
    }
}

impl Engine {
    /// The in-flight workspace allocating this PR's head, if any (the spec
    /// stays until allocate returns, so a second click cannot persist
    /// another worktree).
    pub(crate) fn workspace_creating_from_pr(&self, pr_id: &str) -> Option<Workspace> {
        let pr_id = pr_id.trim();
        if pr_id.is_empty() {
            return None;
        }
        let ids: Vec<String> = self
            .pr_head_checkout
            .lock()
            .iter()
            .filter(|(_, s)| s.pr_id == pr_id)
            .map(|(id, _)| id.clone())
            .collect();
        ids.into_iter()
            .filter_map(|id| self.store.workspace(&id))
            .find(|w| !w.archived)
    }

    /// The PR-head checkout spec for a forge PR (none for a Local PR).
    fn pr_head_spec(&self, pr: &Pr, pr_id: &str, project_id: &str) -> Option<PrHeadCheckoutSpec> {
        let forge = self.pr_forge(pr)?;
        if pr.number <= 0 {
            return None;
        }
        Some(PrHeadCheckoutSpec {
            number: pr.number,
            branch: checkout_pr_head_name(
                &pr.branch,
                &pr.base,
                &self.project_base_branch(project_id),
                pr.is_cross_repository,
            ),
            pr_id: pr_id.to_owned(),
            cross_repo: pr.is_cross_repository,
            head: Some(forge.head_ref(pr.number)),
        })
    }

    /// A Recur workspace on this PR's head, linked to it. A workspace
    /// already on the head (or tracking it) is reused; an in-flight create
    /// is returned; a PR chat host is promoted in place; otherwise a new
    /// worktree is allocated on the head.
    pub async fn create_workspace_from_pr(&self, pr_id: &str) -> Result<Workspace> {
        let pr = self
            .store
            .pr(pr_id)
            .ok_or_else(|| EngineError(format!("PR {pr_id:?} not found")))?;
        let linked = pr.workspace_id.trim();
        if !linked.is_empty()
            && let Some(ws) = self
                .store
                .workspace(linked)
                .filter(|w| !w.archived && !w.is_pr_chat_host())
        {
            self.archive_pr_chat_hosts(pr_id, &ws.id).await;
            return Ok(ws);
        }
        let project_id = self.pr_project_id(&pr);
        if project_id.is_empty() {
            return Err(EngineError(format!("PR {pr_id:?} has no Recur project")));
        }
        if let Some(w) = self
            .workspace_matching_pr_branch(&project_id, &pr.branch)
            .await
        {
            self.store.update_pr(pr_id, |p| {
                p.workspace_id = w.id.clone();
                p.project_id = project_id.clone();
            });
            self.publish_pr_updated(pr_id);
            self.archive_pr_chat_hosts(pr_id, &w.id).await;
            return Ok(w);
        }
        if let Some(w) = self.workspace_creating_from_pr(pr_id) {
            self.archive_pr_chat_hosts(pr_id, &w.id).await;
            return Ok(w);
        }
        if let Some(host) = self.pr_chat_host(pr_id) {
            return Ok(self.promote_pr_chat_host(host, &pr));
        }
        let mut base = pr.base.trim().to_owned();
        if base.is_empty() {
            base = self.project_base_branch(&project_id);
        }
        let base = self.local_base_from_forge(&project_id, &base).await;
        let rec = self
            .persist_creating_workspace(&project_id, &pr_workspace_name(&pr), &base)
            .await?;
        if let Some(spec) = self.pr_head_spec(&pr, pr_id, &project_id) {
            self.pr_head_checkout.lock().insert(rec.id.clone(), spec);
        }
        self.start_allocate(rec.clone());
        Ok(rec)
    }

    /// A workspace the PR-detail agent dock can bind to: a live linked
    /// workspace wins; otherwise a hidden host on the project checkout is
    /// created (or reused) without linking the PR. Create Workspace later
    /// promotes that host in place.
    pub async fn ensure_pr_chat_host(&self, pr_id: &str) -> Result<Workspace> {
        let pr = self
            .store
            .pr(pr_id)
            .ok_or_else(|| EngineError(format!("PR {pr_id:?} not found")))?;
        let linked = self.live_pr_workspace_id(&pr.workspace_id);
        if !linked.is_empty()
            && let Some(ws) = self.store.workspace(&linked)
        {
            self.archive_pr_chat_hosts(pr_id, &ws.id).await;
            if let Some(host) = self.pr_chat_host(pr_id) {
                return Ok(host);
            }
            return Ok(ws);
        }
        if pr_settled(&pr) {
            return Err(EngineError(format!(
                "PR {pr_id:?} is {} — open an open pull request to chat",
                pr.state
            )));
        }
        let project_id = self.pr_project_id(&pr);
        if project_id.is_empty() {
            return Err(EngineError(format!("PR {pr_id:?} has no Recur project")));
        }
        if let Some(w) = self.workspace_creating_from_pr(pr_id) {
            return Ok(w);
        }
        if let Some(w) = self
            .workspace_matching_pr_branch(&project_id, &pr.branch)
            .await
        {
            self.store.update_pr(pr_id, |p| {
                p.workspace_id = w.id.clone();
                p.project_id = project_id.clone();
            });
            self.publish_pr_updated(pr_id);
            self.archive_pr_chat_hosts(pr_id, &w.id).await;
            return Ok(w);
        }
        let saved = {
            let _id = self.id_mu.lock().await;
            if let Some(host) = self.pr_chat_host(pr_id) {
                return Ok(host);
            }
            let proj = self
                .store
                .project(&project_id)
                .ok_or_else(|| EngineError(format!("project {project_id:?} not found")))?;
            let root = proj.repo_path.trim().to_owned();
            if root.is_empty() {
                return Err(EngineError(format!(
                    "project {project_id:?} has no repo path"
                )));
            }
            if !recur_git::is_repo(&root).await {
                return Err(EngineError(format!(
                    "project {project_id:?} is not a git repository — PR chat needs a git checkout"
                )));
            }
            let name = pr_workspace_name(&pr);
            let mut base = pr.base.trim().to_owned();
            if base.is_empty() {
                base = proj.base_branch.clone();
            }
            let base = self.local_base_from_forge(&project_id, &base).await;
            let mgr = self.git_manager(&root);
            let id = self.mint_workspace_id(&project_id, Some(&mgr)).await;
            let rec = Workspace {
                id,
                project_id: project_id.clone(),
                r#type: WORKSPACE_SOLO.into(),
                group: Group::Active,
                goal: name.clone(),
                short: truncate(&name, 48),
                status_label: "Review".into(),
                phase: "editing".into(),
                host_pr_id: pr_id.to_owned(),
                worktree_path: root,
                base_branch: base,
                last: "pr chat".into(),
                elapsed_label: "just now".into(),
                ..Workspace::default()
            };
            self.store
                .add_workspace(rec)
                .map_err(|e| EngineError(e.to_string()))?
        };
        self.ensure_pair_agent(&saved.id, PAIR_AGENT_CODE, "");
        self.remember_workspace_chat_selection(
            &saved.id,
            PAIR_AGENT_CODE,
            "",
            None,
            CHAT_MODE_REVIEW,
            None,
        )
        .await;
        self.publish_loop_updated(
            &saved.id,
            saved.state.clone(),
            &saved.phase,
            saved.iteration,
        );
        Ok(saved)
    }

    /// Turns a chat host into a real PR worktree in place so sessions and
    /// tabs keep the workspace id. `host_pr_id` stays set until allocate
    /// writes the new path (tearing the host down must never remove the
    /// project checkout); the PR is linked once the worktree lands.
    fn promote_pr_chat_host(&self, mut host: Workspace, pr: &Pr) -> Workspace {
        let mut pr_id = pr.id.trim().to_owned();
        if pr_id.is_empty() {
            pr_id = host.host_pr_id.trim().to_owned();
        }
        let mut project_id = self.pr_project_id(pr);
        if project_id.is_empty() {
            project_id = host.project_id.clone();
        }
        if let Some(updated) = self.store.update_workspace(&host.id, |w| {
            w.phase = PHASE_CREATING.into();
            w.status_label = STATUS_CREATING.into();
            w.last_error.clear();
            w.last = "creating".into();
        }) {
            host = updated;
            self.publish_loop_updated(&host.id, host.state.clone(), &host.phase, host.iteration);
        }
        if let Some(spec) = self.pr_head_spec(pr, &pr_id, &project_id) {
            self.pr_head_checkout.lock().insert(host.id.clone(), spec);
        }
        self.start_allocate(host.clone());
        host
    }
}
