//! `PRsService`: the dashboard, detail refreshes, review actions and merges
//! over the engine, with the error text for each `(T, ok)` the engine returns.

use async_trait::async_trait;
use recur_domain::{
    Commit, FileDiff, Finding, MergeMethod, Pr, PrComment, PrParticipant, UpdateWorkspaceResult,
    Workspace,
};

use super::rt::rt;
use super::{AppServices, err};
use crate::api::PrsService;
use crate::{Error, Result};

fn pr_not_found(id: &str) -> Error {
    Error::new(format!("PR {:?} not found", id))
}

impl AppServices {
    /// `msg` when the PR exists (the forge call failed), else not found.
    fn pr_failed(&self, pr_id: &str, msg: &str) -> Error {
        if self.store().pr(pr_id).is_some() {
            Error::new(msg)
        } else {
            pr_not_found(pr_id)
        }
    }

    fn conversation_locked(&self, pr_id: &str) -> bool {
        self.store()
            .pr(pr_id)
            .is_some_and(|p| p.conversation_locked())
    }

    /// Projects open findings onto the diff, writing through when anything
    /// changed.
    fn ensure_pr_enriched(&self, p: &mut Pr) {
        let findings = self
            .store()
            .findings(&self.engine.pr_findings_workspace_id(p));
        if !recur_code::observe::project_findings_onto_diff(
            p.files.as_deref_mut().unwrap_or_default(),
            &findings,
        ) {
            return;
        }
        let (files, commits, impact) = (p.files.clone(), p.commits.clone(), p.impact.clone());
        self.store().update_pr(&p.id, |pr| {
            pr.files = files;
            pr.commits = commits;
            if !impact.is_empty() {
                pr.impact = impact;
            }
        });
    }

    async fn get_pr_at(&self, id: &str) -> Result<Pr> {
        if self.store().pr(id).is_none() {
            return Err(pr_not_found(id));
        }
        // Conversation and diff start first, in the background: Files
        // changed must not wait on the fetches below. Behind must finish
        // before commits: it fetches a stale remote-tracking base the commit
        // log then reads. Checks are independent.
        self.engine.schedule_refresh_pr_diff(id);
        self.engine.schedule_refresh_pr_conversation(id);
        let commits = async {
            self.engine.refresh_pr_behind(id).await;
            self.engine.refresh_pr_commits(id).await;
        };
        tokio::join!(commits, self.engine.refresh_pr_checks(id));
        let mut p = self.store().pr(id).unwrap_or_default();
        self.ensure_pr_enriched(&mut p);
        // The checks poll calls this every 15s and per-commit diffs can be
        // tens of MB: the UI loads them lazily through GetCommitDiff.
        for c in p.commits.iter_mut().flatten() {
            c.files = None;
        }
        Ok(p)
    }

    fn enriched_pr(&self, id: &str) -> Pr {
        let mut p = self.store().pr(id).unwrap_or_default();
        self.ensure_pr_enriched(&mut p);
        p
    }
}

#[async_trait]
impl PrsService for AppServices {
    async fn add_pr_comment(
        &self,
        pr_id: &str,
        file: &str,
        line: i64,
        body: &str,
        start_line: i64,
        finding_id: &str,
    ) -> Result<PrComment> {
        rt!(self, pr_id, file, line, body, start_line, finding_id => |s| {
            if s.conversation_locked(&pr_id) && !body.trim().is_empty() {
                return Err(Error::new("this pull request is closed"));
            }
            let (c, ok) = s
                .engine
                .add_pr_comment(&pr_id, &file, line, &body, start_line, &finding_id)
                .await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not post the comment to GitHub"));
            }
            Ok(c)
        })
    }

    async fn close_pr(&self, pr_id: &str) -> Result<Pr> {
        rt!(self, pr_id => |s| {
            let (p, ok) = s.engine.close_pr(&pr_id).await;
            if !ok {
                if p.id.is_empty() {
                    return Err(pr_not_found(&pr_id));
                }
                return Err(Error::new("could not close the PR"));
            }
            Ok(p)
        })
    }

    async fn create_workspace_from_pr(&self, pr_id: &str) -> Result<Workspace> {
        rt!(self, pr_id => |s| s.engine.create_workspace_from_pr(&pr_id).await.map_err(err))
    }

    async fn delete_pr_comment(&self, pr_id: &str, comment_id: &str) -> Result<Pr> {
        rt!(self, pr_id, comment_id => |s| {
            let (p, ok) = s.engine.delete_pr_comment(&pr_id, &comment_id).await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not delete the comment"));
            }
            Ok(p)
        })
    }

    async fn ensure_pr_chat_host(&self, pr_id: &str) -> Result<Workspace> {
        rt!(self, pr_id => |s| s.engine.ensure_pr_chat_host(&pr_id).await.map_err(err))
    }

    async fn get_commit_diff(&self, id: &str, hash: &str) -> Result<Commit> {
        rt!(self, id, hash => |s| s.engine.ensure_commit_diff(&id, &hash).await.map_err(err))
    }

    async fn get_pr(&self, id: &str) -> Result<Pr> {
        rt!(self, id => |s| s.get_pr_at(&id).await)
    }

    async fn get_pr_commits(&self, id: &str) -> Result<Vec<Commit>> {
        rt!(self, id => |s| Ok(s.enriched_pr(&id).commits.unwrap_or_default()))
    }

    async fn get_pr_files(&self, id: &str) -> Result<Vec<FileDiff>> {
        rt!(self, id => |s| Ok(s.enriched_pr(&id).files.unwrap_or_default()))
    }

    async fn list_pr_findings(&self, pr_id: &str) -> Result<Vec<Finding>> {
        rt!(self, pr_id => |s| {
            let Some(p) = s.store().pr(&pr_id) else {
                return Ok(Vec::new());
            };
            let id = s.engine.pr_findings_workspace_id(&p);
            if id.is_empty() {
                return Ok(Vec::new());
            }
            Ok(s.store().findings(&id))
        })
    }

    async fn list_prs(&self) -> Result<Vec<Pr>> {
        rt!(self, => |s| Ok(s.store().prs()))
    }

    async fn merge_pr(&self, pr_id: &str, method: &str) -> Result<Pr> {
        rt!(self, pr_id, method => |s| {
            if s.store().pr(&pr_id).is_none() {
                return Err(pr_not_found(&pr_id));
            }
            s.engine
                .merge_pr(&pr_id, &MergeMethod::from(method))
                .await
                .map_err(|note| Error::new(note_text(&note)))
        })
    }

    async fn ready_pr(&self, pr_id: &str) -> Result<Pr> {
        self.set_pr_draft(pr_id, false).await
    }

    async fn reply_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<PrComment> {
        rt!(self, pr_id, comment_id, body => |s| {
            if s.conversation_locked(&pr_id) {
                return Err(Error::new("this pull request is closed"));
            }
            let (c, ok) = s.engine.reply_pr_comment(&pr_id, &comment_id, &body).await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not reply to the comment"));
            }
            Ok(c)
        })
    }

    async fn search_pr_mentionable_users(
        &self,
        pr_id: &str,
        query: &str,
    ) -> Result<Vec<PrParticipant>> {
        rt!(self, pr_id, query => |s| Ok(s.engine.search_pr_mentionable_users(&pr_id, &query).await))
    }

    async fn send_back_to_loop(&self, pr_id: &str, comments: &[String]) -> Result<Pr> {
        rt!(self, pr_id, comments => |s| {
            let (p, ok) = s.engine.send_back_to_loop(&pr_id, &comments).await;
            if !ok {
                return Err(Error::new(
                    "send-back failed: the PR or its workflow is unavailable",
                ));
            }
            Ok(p)
        })
    }

    async fn set_pr_draft(&self, pr_id: &str, draft: bool) -> Result<Pr> {
        rt!(self, pr_id, draft => |s| {
            let (p, ok) = s.engine.set_pr_draft(&pr_id, draft).await;
            if !ok {
                if p.id.is_empty() {
                    return Err(pr_not_found(&pr_id));
                }
                return Err(Error::new(if draft {
                    "could not convert the PR to a draft"
                } else {
                    "could not mark the PR ready for review"
                }));
            }
            Ok(p)
        })
    }

    async fn set_pr_thread_resolved(
        &self,
        pr_id: &str,
        thread_id: &str,
        resolved: bool,
    ) -> Result<Pr> {
        rt!(self, pr_id, thread_id, resolved => |s| {
            let (p, ok) = s.engine.set_pr_thread_resolved(&pr_id, &thread_id, resolved).await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not update the conversation"));
            }
            Ok(p)
        })
    }

    async fn submit_review(&self, pr_id: &str, kind: &str, body: &str) -> Result<Pr> {
        rt!(self, pr_id, kind, body => |s| {
            if s.conversation_locked(&pr_id) {
                return Err(Error::new("this pull request is closed"));
            }
            let (p, ok) = s.engine.submit_review(&pr_id, &kind, &body).await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not submit the review to GitHub"));
            }
            Ok(p)
        })
    }

    async fn sync_github_prs(&self) -> Result<Vec<Pr>> {
        rt!(self, => |s| Ok(s.engine.sync_prs().await))
    }

    async fn toggle_pr_comment_reaction(
        &self,
        pr_id: &str,
        comment_id: &str,
        content: &str,
    ) -> Result<Pr> {
        rt!(self, pr_id, comment_id, content => |s| {
            let (p, ok) = s
                .engine
                .toggle_pr_comment_reaction(&pr_id, &comment_id, &content)
                .await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not update the reaction"));
            }
            Ok(p)
        })
    }

    async fn update_pr_comment(
        &self,
        pr_id: &str,
        comment_id: &str,
        body: &str,
    ) -> Result<PrComment> {
        rt!(self, pr_id, comment_id, body => |s| {
            let (c, ok) = s.engine.update_pr_comment(&pr_id, &comment_id, &body).await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not update the comment"));
            }
            Ok(c)
        })
    }

    async fn update_pr_details(&self, pr_id: &str, title: &str, body: &str) -> Result<Pr> {
        rt!(self, pr_id, title, body => |s| {
            let (p, ok) = s.engine.update_pr_details(&pr_id, &title, &body).await;
            if !ok {
                return Err(s.pr_failed(&pr_id, "could not update the pull request"));
            }
            Ok(p)
        })
    }

    async fn update_with_base(&self, pr_id: &str, method: &str) -> Result<UpdateWorkspaceResult> {
        rt!(self, pr_id, method => |s| s.engine.update_pr_with_base(&pr_id, &method).await.map_err(err))
    }
}

/// A feed note's HTML as toast text: tags dropped, entities decoded.
fn note_text(html: &str) -> String {
    recur_engine::text::html_unescape(&recur_engine::loops::maestro::strip_tags(html))
}

#[cfg(test)]
mod tests {
    #[test]
    fn note_text_drops_tags_and_decodes_entities() {
        assert_eq!(
            super::note_text("GitHub merge failed: <code>a &lt;b&gt; &#34;c&#34; &amp;amp;</code>"),
            "GitHub merge failed: a <b> \"c\" &amp;"
        );
    }
}
