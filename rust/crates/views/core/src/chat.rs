//! The chat stores several views share (`lib/stores.ts`): the live terminal and artifact lists
//! behind `@terminal` / `@artifact` mentions, one-shot focus requests between the chat and the
//! terminal dock or a fan-out card, the model catalog cache (`lib/models.ts`), and whether an
//! agent transcript is the on-screen chat surface.

use futures::FutureExt;
use futures::future::Shared;
use gpui_kit::{AppContext, Context, Task};

use crate::models::{CatalogModel, flatten_model_catalog};
use crate::nav::ViewName;
use crate::pr::pr_agent_workspace_id;
use crate::state::AppState;

/// A live terminal tab for `@terminal` mentions (`WorkspaceTerminalTab`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceTerminalTab {
    pub id: String,
    /// The OSC title.
    pub title: String,
    pub exited: bool,
    /// The focused pane.
    pub active: bool,
}

/// A workspace artifact for `@artifact` mentions (`WorkspaceArtifact`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceArtifact {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub path: String,
}

/// A request for the terminal dock to focus a session (`terminalFocus`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalFocus {
    pub workspace_id: String,
    pub id: String,
}

/// The wire answer to an `ask_user` question (`AskUserAnswerWire`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AskUserAnswer {
    pub choice: String,
    pub free_text: String,
    pub attachments: Vec<recur_domain::ChatAttachment>,
}

pub(crate) type CatalogLoad = Shared<Task<Vec<CatalogModel>>>;

/// The model catalog cache: the last good load, the request in flight and the revision
/// `invalidateModelCatalog` bumps.
#[derive(Default)]
pub struct ModelCatalogCache {
    pub(crate) models: Option<Vec<CatalogModel>>,
    pub(crate) inflight: Option<(u64, CatalogLoad)>,
    pub(crate) generation: u64,
    pub rev: u64,
}

impl AppState {
    /// `answerPermission`: drop the card at once, then answer. A failure means it was already
    /// resolved elsewhere, timed out, or the turn ended.
    pub fn answer_permission(
        &mut self,
        id: &str,
        allow: bool,
        reason: &str,
        remember: bool,
        cx: &mut Context<Self>,
    ) {
        self.permission_requests.retain(|r| r.id != id);
        cx.notify();
        let services = self.services.clone();
        let (id, reason) = (id.to_string(), reason.to_string());
        cx.background_spawn(async move {
            let _ = services
                .permissions
                .answer_permission(&id, allow, &reason, remember)
                .await;
        })
        .detach();
    }

    /// `answerQuestion`: drop the card so a double click cannot resubmit, tell listeners
    /// before the RPC (the resume turn can stream while it is pending), and toast a rejection
    /// without resurrecting the card. Resolves whether the backend took the answer.
    pub fn answer_question(
        &mut self,
        id: &str,
        answer: AskUserAnswer,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let question = self.user_questions.iter().find(|q| q.id == id).cloned();
        self.user_questions.retain(|q| q.id != id);
        cx.notify();
        if let Some(q) = &question {
            cx.emit(crate::ShellEvent::QuestionAnswered(Box::new((
                q.clone(),
                answer.clone(),
            ))));
        }
        let services = self.services.clone();
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let res = services
                .questions
                .answer_question(&id, &answer.choice, &answer.free_text, &answer.attachments)
                .await;
            match res {
                Ok(()) => true,
                Err(err) => {
                    let _ = this.update(cx, |_, cx| {
                        if let Some(q) = question {
                            cx.emit(crate::ShellEvent::QuestionAnswerFailed(Box::new((
                                q, answer,
                            ))));
                        }
                        let msg = crate::bridge_error_message(&err);
                        crate::toast(
                            recur_ui::widgets::toasts::ToastKind::Error,
                            if msg.is_empty() {
                                "That question is no longer waiting".to_string()
                            } else {
                                msg
                            },
                            cx,
                        );
                    });
                    false
                }
            }
        })
    }

    /// `markWorkspaceInboxRead`: every unread notification of a workspace, read.
    pub fn mark_workspace_inbox_read(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        if workspace_id.is_empty() {
            return;
        }
        let ids: Vec<String> = self
            .inbox
            .iter()
            .filter(|i| i.workspace_id == workspace_id && i.unread)
            .map(|i| i.id.clone())
            .collect();
        if ids.is_empty() {
            return;
        }
        let services = self.services.clone();
        cx.background_spawn(async move {
            for id in ids {
                let _ = services.inbox.mark_inbox_read(&id).await;
            }
        })
        .detach();
        for i in self
            .inbox
            .iter_mut()
            .filter(|i| i.workspace_id == workspace_id)
        {
            i.unread = false;
        }
        cx.notify();
    }

    /// `workspaceTerminals[id]`.
    pub fn workspace_terminals(&self, workspace_id: &str) -> &[WorkspaceTerminalTab] {
        self.chat
            .terminals
            .get(workspace_id)
            .map_or(&[], Vec::as_slice)
    }

    /// Replaces a workspace's terminal list; identical lists do not notify.
    pub fn set_workspace_terminals(
        &mut self,
        workspace_id: &str,
        tabs: Vec<WorkspaceTerminalTab>,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_terminals(workspace_id) == tabs.as_slice() {
            return;
        }
        self.chat.terminals.insert(workspace_id.to_string(), tabs);
        cx.notify();
    }

    /// `workspaceArtifacts[id]`.
    pub fn workspace_artifacts(&self, workspace_id: &str) -> &[WorkspaceArtifact] {
        self.chat
            .artifacts
            .get(workspace_id)
            .map_or(&[], Vec::as_slice)
    }

    /// Replaces a workspace's artifact list; identical lists do not notify.
    pub fn set_workspace_artifacts(
        &mut self,
        workspace_id: &str,
        list: Vec<WorkspaceArtifact>,
        cx: &mut Context<Self>,
    ) {
        if self.workspace_artifacts(workspace_id) == list.as_slice() {
            return;
        }
        self.chat.artifacts.insert(workspace_id.to_string(), list);
        cx.notify();
    }

    /// An `artifact.created` row: replaces the same id in place, else goes first.
    pub fn upsert_workspace_artifact(
        &mut self,
        workspace_id: &str,
        row: WorkspaceArtifact,
        cx: &mut Context<Self>,
    ) {
        let list = self
            .chat
            .artifacts
            .entry(workspace_id.to_string())
            .or_default();
        match list.iter_mut().find(|a| a.id == row.id) {
            Some(slot) => *slot = row,
            None => list.insert(0, row),
        }
        cx.notify();
    }

    /// `terminalFocus.set`: the dock should focus this session.
    pub fn request_terminal_focus(&mut self, workspace_id: &str, id: &str, cx: &mut Context<Self>) {
        self.chat.terminal_focus = Some(TerminalFocus {
            workspace_id: workspace_id.to_string(),
            id: id.to_string(),
        });
        cx.notify();
    }

    pub fn terminal_focus(&self) -> Option<&TerminalFocus> {
        self.chat.terminal_focus.as_ref()
    }

    /// Claims a pending terminal focus request for `workspace_id`, once.
    pub fn take_terminal_focus(&mut self, workspace_id: &str) -> Option<String> {
        if self.chat.terminal_focus.as_ref()?.workspace_id != workspace_id {
            return None;
        }
        self.chat.terminal_focus.take().map(|f| f.id)
    }

    /// `childTranscriptRequest.set`: expand a settled child's transcript in its wave card.
    pub fn request_child_transcript(&mut self, child_id: &str, cx: &mut Context<Self>) {
        self.chat.child_transcript = Some(child_id.to_string());
        cx.notify();
    }

    pub fn child_transcript_request(&self) -> Option<&str> {
        self.chat.child_transcript.as_deref()
    }

    /// `childTranscriptRequest.set(null)` by the card that owns the child.
    pub fn clear_child_transcript_request(&mut self) {
        self.chat.child_transcript = None;
    }

    /// `agentChatOnScreen`: the workspace detail, or PR detail with its agent dock open on a
    /// linked or hosting workspace.
    pub fn agent_chat_on_screen(&self) -> bool {
        match self.view {
            ViewName::Detail => true,
            ViewName::PrDetail => {
                let Some(pr) = self.pr(&self.selected_pr_id) else {
                    return false;
                };
                self.pr_agent_dock_open
                    && !pr_agent_workspace_id(&pr.id, &pr.workspace_id, &self.workspaces).is_empty()
            }
            _ => false,
        }
    }

    /// `peekModelCatalog`: the last good catalog (empty before the first load).
    pub fn peek_model_catalog(&self) -> &[CatalogModel] {
        self.chat.catalog.models.as_deref().unwrap_or(&[])
    }

    /// `modelCatalogRev`.
    pub fn model_catalog_rev(&self) -> u64 {
        self.chat.catalog.rev
    }

    /// `invalidateModelCatalog`: drop the cache so the next load hits the server, and bump the
    /// revision so open pickers re-fetch.
    pub fn invalidate_model_catalog(&mut self, cx: &mut Context<Self>) {
        self.chat.catalog.models = None;
        self.chat.catalog.inflight = None;
        self.chat.catalog.generation += 1;
        self.chat.catalog.rev += 1;
        cx.notify();
    }

    /// `loadModelCatalog`: the cached catalog, or one shared request. A request superseded by
    /// an invalidation does not overwrite the fresh cache. Failures yield an empty list.
    pub fn load_model_catalog(&mut self, cx: &mut Context<Self>) -> Task<Vec<CatalogModel>> {
        if let Some(models) = self.chat.catalog.models.clone() {
            return Task::ready(models);
        }
        let generation = self.chat.catalog.generation;
        if let Some((g, load)) = &self.chat.catalog.inflight
            && *g == generation
        {
            let load = load.clone();
            return cx.background_spawn(load);
        }
        let services = self.services.clone();
        let fetch: Task<Vec<CatalogModel>> = cx.background_spawn(async move {
            services
                .settings
                .list_model_catalog()
                .await
                .map(|groups| flatten_model_catalog(&groups))
                .unwrap_or_default()
        });
        let shared = fetch.shared();
        self.chat.catalog.inflight = Some((generation, shared.clone()));
        cx.spawn(async move |this, cx| {
            let models = shared.await;
            let _ = this.update(cx, |s, cx| {
                if s.chat.catalog.generation == generation {
                    s.chat.catalog.models = Some(models.clone());
                    s.chat.catalog.inflight = None;
                    cx.notify();
                }
            });
            models
        })
    }
}

/// The per-window chat stores held by [`AppState`].
#[derive(Default)]
pub struct ChatStores {
    pub(crate) terminals: std::collections::BTreeMap<String, Vec<WorkspaceTerminalTab>>,
    pub(crate) artifacts: std::collections::BTreeMap<String, Vec<WorkspaceArtifact>>,
    pub(crate) terminal_focus: Option<TerminalFocus>,
    pub(crate) child_transcript: Option<String>,
    pub(crate) catalog: ModelCatalogCache,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::TestAppContext;
    use recur_services::api::{ModelChoice, ModelGroup};
    use recur_services::mock::{MockBridge, MockOptions, Reply};
    use serde_json::json;

    use crate::{AppState, LocalStorage};

    fn group(provider: &str, refs: &[&str]) -> ModelGroup {
        ModelGroup {
            provider: provider.into(),
            label: provider.into(),
            models: refs
                .iter()
                .map(|r| ModelChoice {
                    model_ref: r.to_string(),
                    label: r.to_string(),
                    ..Default::default()
                })
                .collect(),
        }
    }

    // models-catalog.test.ts
    #[gpui::test]
    async fn invalidate_bumps_the_revision_and_reloads(cx: &mut TestAppContext) {
        let groups = Arc::new(parking_lot::Mutex::new(vec![group(
            "claude-code",
            &["claude-code/a"],
        )]));
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (g, c) = (groups.clone(), calls.clone());
        let bridge = MockBridge::fresh(MockOptions::default().resolver(
            "ListModelCatalog",
            move |_, _, _| {
                c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Reply::value(json!(g.lock().clone()))
            },
        ));
        let state =
            cx.update(|cx| AppState::install(bridge.services(), LocalStorage::in_memory(), cx));
        let first = state.update(cx, |s, cx| s.load_model_catalog(cx)).await;
        assert_eq!(first[0].r#ref, "claude-code/a");
        let rev = state.read_with(cx, |s, _| s.model_catalog_rev());
        groups
            .lock()
            .push(group("local-ollama", &["local-ollama/q"]));
        state.update(cx, |s, cx| s.invalidate_model_catalog(cx));
        assert_eq!(state.read_with(cx, |s, _| s.model_catalog_rev()), rev + 1);
        let next = state.update(cx, |s, cx| s.load_model_catalog(cx)).await;
        let refs: Vec<_> = next.iter().map(|m| m.r#ref.as_str()).collect();
        assert_eq!(refs, ["claude-code/a", "local-ollama/q"]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        // Cached: no third call.
        let _ = state.update(cx, |s, cx| s.load_model_catalog(cx)).await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[gpui::test]
    async fn a_superseded_request_does_not_overwrite_the_fresh_cache(cx: &mut TestAppContext) {
        let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = n.clone();
        let bridge = MockBridge::fresh(MockOptions::default().resolver(
            "ListModelCatalog",
            move |_, _, _| {
                let i = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if i == 0 {
                    Reply::value(json!([group("claude-code", &["claude-code/stale"])]))
                } else {
                    Reply::value(json!([group("claude-code", &["claude-code/b"])]))
                }
            },
        ));
        let state =
            cx.update(|cx| AppState::install(bridge.services(), LocalStorage::in_memory(), cx));
        let a = state.update(cx, |s, cx| s.load_model_catalog(cx));
        state.update(cx, |s, cx| s.invalidate_model_catalog(cx));
        let b = state.update(cx, |s, cx| s.load_model_catalog(cx));
        let _ = b.await;
        let _ = a.await;
        let now = state.update(cx, |s, cx| s.load_model_catalog(cx)).await;
        assert_eq!(now[0].r#ref, "claude-code/b");
    }
}
