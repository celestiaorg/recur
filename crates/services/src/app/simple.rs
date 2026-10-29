//! The thin services: agents, artifacts, escalations, permissions, questions,
//! policy, inbox, Maestro, the briefing and Ask Recur, and findings. Each is a
//! delegation to the store or engine with its error text.

use std::collections::BTreeMap;

use async_trait::async_trait;
use recur_domain::{
    Agent, Artifact, BriefingSnapshot, ChatAttachment, ChatItem, Escalation, FINDING_ACTOR_HUMAN,
    FINDING_SOURCE_USER, Finding, FindingStatus, InboxItem, PermissionDecision, PermissionRequest,
    Policy, Severity, UserQuestion, WorkspaceChatTab,
};
use recur_engine::attention::chat_inbox::parse_chat_inbox_id;
use recur_engine::bus::{EngineEvent, InboxUpdated, WorkspaceRef};

use super::rt::rt;
use super::{AppServices, err};
use crate::api::{
    AgentsService, AppChatSelection, ArtifactsService, BriefingService, EscalationService,
    InboxService, MaestroService, ObservatoryService, PermissionsService, PolicyService,
    QuestionsService,
};
use crate::{Error, Result};

impl AppServices {
    fn publish_findings_updated(&self, workspace_id: &str) {
        self.engine
            .bus
            .publish(EngineEvent::FindingsUpdated(WorkspaceRef::new(
                workspace_id,
            )));
    }

    /// Publishes the unread count.
    fn emit_inbox(&self) -> i64 {
        let n = self.store().unread_count() as i64;
        self.engine
            .bus
            .publish(EngineEvent::InboxUpdated(InboxUpdated { unread: n }));
        n
    }

    fn create_finding_sync(
        &self,
        workspace_id: &str,
        file: &str,
        line: i64,
        message: &str,
        end_line: i64,
    ) -> Result<Finding> {
        let (workspace_id, file, message) = (workspace_id.trim(), file.trim(), message.trim());
        if workspace_id.is_empty() {
            return Err(Error::new("workspace id required"));
        }
        if file.is_empty() {
            return Err(Error::new("file required"));
        }
        if message.is_empty() {
            return Err(Error::new("message required"));
        }
        if line < 0 {
            return Err(Error::new("line must be ≥ 0"));
        }
        if end_line < 0 {
            return Err(Error::new("end line must be ≥ 0"));
        }
        let mut end_line = end_line;
        if end_line > 0 {
            if line <= 0 {
                return Err(Error::new("line is required when end line is set"));
            }
            if end_line < line {
                return Err(Error::new("end line must be ≥ line"));
            }
            if end_line == line {
                end_line = 0;
            }
        }
        let Some(ws) = self.store().workspace(workspace_id) else {
            return Err(Error::new(format!(
                "workspace {:?} not found",
                workspace_id
            )));
        };
        let f = self.store().add_finding(Finding {
            workspace_id: workspace_id.to_owned(),
            iteration: ws.iteration.max(1),
            file: file.to_owned(),
            line,
            end_line,
            severity: Severity::Info,
            message: message.to_owned(),
            status: FindingStatus::Open.as_str().to_owned(),
            updated_by: FINDING_ACTOR_HUMAN.into(),
            source: FINDING_SOURCE_USER.into(),
            ..Finding::default()
        });
        self.publish_findings_updated(workspace_id);
        Ok(f)
    }
}

#[async_trait]
impl AgentsService for AppServices {
    async fn get_agent_trace(&self, workspace_id: &str, code: &str) -> Result<Agent> {
        rt!(self, workspace_id, code => |s| {
            s.store().agent(&workspace_id, &code).ok_or_else(|| {
                Error::new(format!(
                    "agent {:?} not found on loop {:?}",
                    code,
                    workspace_id
                ))
            })
        })
    }

    async fn list_loop_agents(&self, workspace_id: &str) -> Result<Vec<Agent>> {
        rt!(self, workspace_id => |s| Ok(s.store().agents(&workspace_id)))
    }
}

#[async_trait]
impl ArtifactsService for AppServices {
    async fn delete_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
        rt!(self, workspace_id, id => |s| s.engine.delete_artifact(&workspace_id, &id).map_err(err))
    }

    async fn list_artifacts(&self, workspace_id: &str) -> Result<Vec<Artifact>> {
        rt!(self, workspace_id => |s| s.engine.list_artifacts(&workspace_id).map_err(err))
    }

    async fn present_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
        rt!(self, workspace_id, id => |s| {
            let msg = s.engine.present_artifact(&workspace_id, &id);
            if msg.starts_with("Artifact not found") || msg.starts_with("Could not") {
                return Err(Error::new(msg));
            }
            Ok(())
        })
    }

    async fn rename_artifact(&self, workspace_id: &str, id: &str, title: &str) -> Result<Artifact> {
        rt!(self, workspace_id, id, title => |s| {
            s.engine.rename_artifact(&workspace_id, &id, &title).map_err(err)
        })
    }

    async fn reveal_artifact(&self, workspace_id: &str, id: &str) -> Result<()> {
        rt!(self, workspace_id, id => |s| {
            let full = s.engine.artifact_abs_path(&workspace_id, &id).map_err(err)?;
            recur_code::reveal_in_finder(&full.to_string_lossy()).map_err(err)
        })
    }
}

#[async_trait]
impl EscalationService for AppServices {
    async fn answer_escalation(
        &self,
        workspace_id: &str,
        choice: &str,
        free_text: &str,
        agent_code: &str,
    ) -> Result<Escalation> {
        rt!(self, workspace_id, choice, free_text, agent_code => |s| {
            s.engine
                .answer_escalation_for(&workspace_id, &agent_code, &choice, &free_text)
                .await
                .ok_or_else(|| {
                    Error::new(format!(
                        "escalation for loop {:?} is missing or already answered",
                        workspace_id
                    ))
                })
        })
    }

    /// Most workspaces have no escalation. The UI probes this while loading
    /// each workspace, so absence is an ordinary empty state, not an error.
    async fn get_escalation(&self, workspace_id: &str) -> Result<Escalation> {
        rt!(self, workspace_id => |s| Ok(s.store().escalation(&workspace_id).unwrap_or_default()))
    }
}

#[async_trait]
impl PermissionsService for AppServices {
    async fn answer_permission(
        &self,
        id: &str,
        allow: bool,
        reason: &str,
        remember: bool,
    ) -> Result<()> {
        rt!(self, id, allow, reason, remember => |s| {
            let ok = s.engine.answer_permission(
                &id,
                PermissionDecision {
                    allow,
                    reason,
                    remember,
                },
            );
            if !ok {
                return Err(Error::new(format!(
                    "permission request {:?} is no longer waiting",
                    id
                )));
            }
            Ok(())
        })
    }

    async fn list_pending_permissions(&self) -> Result<Vec<PermissionRequest>> {
        rt!(self, => |s| Ok(s.engine.pending_permissions()))
    }
}

#[async_trait]
impl QuestionsService for AppServices {
    async fn answer_question(
        &self,
        id: &str,
        choice: &str,
        free_text: &str,
        attachments: &[ChatAttachment],
    ) -> Result<()> {
        rt!(self, id, choice, free_text, attachments => |s| {
            if !s.engine.answer_ask_user(&id, &choice, &free_text, attachments) {
                return Err(Error::new(format!(
                    "question {:?} is no longer waiting",
                    id
                )));
            }
            Ok(())
        })
    }

    async fn list_pending_questions(&self) -> Result<Vec<UserQuestion>> {
        rt!(self, => |s| Ok(s.engine.pending_ask_users()))
    }
}

#[async_trait]
impl PolicyService for AppServices {
    async fn get_policy(&self) -> Result<Policy> {
        rt!(self, => |s| Ok(s.store().policy()))
    }

    async fn set_policy(&self, p: &Policy) -> Result<Policy> {
        rt!(self, p => |s| {
            s.store().set_policy(p.clone());
            Ok(p)
        })
    }
}

#[async_trait]
impl InboxService for AppServices {
    async fn get_inbox(&self) -> Result<Vec<InboxItem>> {
        rt!(self, => |s| Ok(s.store().inbox().unwrap_or_default()))
    }

    async fn mark_all_inbox_read(&self) -> Result<i64> {
        rt!(self, => |s| {
            s.engine.mark_all_chats_seen();
            s.store().mark_all_inbox_read();
            Ok(s.emit_inbox())
        })
    }

    async fn mark_inbox_read(&self, id: &str) -> Result<i64> {
        rt!(self, id => |s| {
            if let Some((ws, code)) = parse_chat_inbox_id(&id) {
                s.engine.mark_workspace_chat_seen(&ws, &code);
                // Unconditional: marking seen is a no-op when the agent is
                // gone, but the row must still be dismissable.
                s.store().remove_inbox_item(&id);
                return Ok(s.emit_inbox());
            }
            s.store().mark_inbox_read(&id);
            Ok(s.emit_inbox())
        })
    }
}

#[async_trait]
impl MaestroService for AppServices {
    async fn cancel_maestro_message(&self, workspace_id: &str) -> Result<()> {
        rt!(self, workspace_id => |s| {
            s.engine.cancel_chat(&workspace_id, "MA").await;
            Ok(())
        })
    }

    async fn get_maestro_chat(&self, workspace_id: &str) -> Result<Vec<ChatItem>> {
        rt!(self, workspace_id => |s| Ok(s.engine.maestro_chat_items(&workspace_id).await))
    }

    async fn send_maestro_message(
        &self,
        workspace_id: &str,
        text: &str,
        attachments: &[ChatAttachment],
    ) -> Result<String> {
        rt!(self, workspace_id, text, attachments => |s| {
            Ok(s.engine
                .send_maestro_message(&workspace_id, &text, &attachments)
                .await)
        })
    }
}

#[async_trait]
impl BriefingService for AppServices {
    async fn app_chat_items(&self, code: &str) -> Result<Vec<ChatItem>> {
        rt!(self, code => |s| Ok(s.engine.app_chat_items(&code)))
    }

    async fn app_chat_selection(&self, code: &str) -> Result<AppChatSelection> {
        rt!(self, code => |s| {
            let (model, flags) = s.engine.app_chat_selection(&code);
            Ok(AppChatSelection { model, flags })
        })
    }

    async fn cancel_app_message(&self, code: &str) -> Result<()> {
        rt!(self, code => |s| {
            s.engine.cancel_app_message(&code).await;
            Ok(())
        })
    }

    async fn close_app_chat(&self, code: &str) -> Result<Vec<WorkspaceChatTab>> {
        rt!(self, code => |s| Ok(s.engine.close_app_chat(&code)))
    }

    async fn get_briefing(&self) -> Result<BriefingSnapshot> {
        rt!(self, => |s| Ok(s.engine.briefing_snapshot()))
    }

    async fn list_app_chats(&self) -> Result<Vec<WorkspaceChatTab>> {
        rt!(self, => |s| Ok(s.engine.list_app_chats()))
    }

    async fn mark_app_chat_seen(&self, code: &str) -> Result<()> {
        rt!(self, code => |s| {
            s.engine.mark_app_chat_seen(&code);
            Ok(())
        })
    }

    async fn mark_workspace_viewed(
        &self,
        workspace_id: &str,
        seen_through_activity_at: i64,
    ) -> Result<()> {
        rt!(self, workspace_id, seen_through_activity_at => |s| {
            s.engine
                .mark_workspace_viewed(&workspace_id, seen_through_activity_at);
            Ok(())
        })
    }

    async fn new_app_chat(&self) -> Result<WorkspaceChatTab> {
        rt!(self, => |s| Ok(s.engine.new_app_chat()))
    }

    async fn remember_app_chat_selection(
        &self,
        code: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
    ) -> Result<()> {
        rt!(self, code, model, flags => |s| {
            s.engine
                .remember_app_chat_selection(&code, &model, Some(&flags));
            Ok(())
        })
    }

    async fn send_app_message(
        &self,
        code: &str,
        text: &str,
        model: &str,
        flags: &BTreeMap<String, String>,
    ) -> Result<String> {
        rt!(self, code, text, model, flags => |s| {
            Ok(s.engine
                .send_app_message(&code, &text, &model, Some(&flags))
                .await)
        })
    }

    async fn set_app_chat_focus(&self, visible: bool, code: &str) -> Result<()> {
        rt!(self, visible, code => |s| {
            s.engine.set_app_chat_focus(visible, &code);
            Ok(())
        })
    }
}

#[async_trait]
impl ObservatoryService for AppServices {
    async fn add_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        body: &str,
    ) -> Result<Finding> {
        rt!(self, workspace_id, finding_id, body => |s| {
            let f = s
                .store()
                .add_finding_note(&workspace_id, &finding_id, &body, FINDING_ACTOR_HUMAN)
                .map_err(err)?;
            s.publish_findings_updated(&workspace_id);
            Ok(f)
        })
    }

    async fn create_finding(
        &self,
        workspace_id: &str,
        file: &str,
        line: i64,
        message: &str,
        end_line: i64,
    ) -> Result<Finding> {
        rt!(self, workspace_id, file, line, message, end_line => |s| {
            s.create_finding_sync(&workspace_id, &file, line, &message, end_line)
        })
    }

    async fn delete_finding_note(
        &self,
        workspace_id: &str,
        finding_id: &str,
        note_id: &str,
    ) -> Result<Finding> {
        rt!(self, workspace_id, finding_id, note_id => |s| {
            let f = s
                .store()
                .delete_finding_note(&workspace_id, &finding_id, &note_id)
                .map_err(err)?;
            s.publish_findings_updated(&workspace_id);
            Ok(f)
        })
    }

    async fn list_findings(&self, workspace_id: &str) -> Result<Vec<Finding>> {
        rt!(self, workspace_id => |s| Ok(s.store().findings(&workspace_id)))
    }

    async fn set_finding_status(
        &self,
        workspace_id: &str,
        finding_id: &str,
        status: &str,
    ) -> Result<Finding> {
        rt!(self, workspace_id, finding_id, status => |s| {
            let f = s
                .store()
                .set_finding_status(&workspace_id, &finding_id, &status)
                .map_err(err)?;
            s.publish_findings_updated(&workspace_id);
            Ok(f)
        })
    }
}
