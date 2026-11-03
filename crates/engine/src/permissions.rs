//! The human permission broker: agent permission prompts reach a human and
//! block the asking MCP handler (or Codex approval) until an answer, the turn
//! ends, or the 10-minute timeout denies.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use recur_agent::PermissionAbandoned;
use recur_agent_tools::{HostError, PermissionAsk, PermissionVerdict};
use recur_domain::{Group, InboxItem, PermissionDecision, PermissionRequest};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::Engine;
use crate::bus::{EngineEvent, PermissionResolved};
use crate::codes::chat_turn_key;
use crate::text::unix_nanos;

/// How long an agent's tool call waits on a human before it is denied.
pub const PERMISSION_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// The UI asks inline in the chat thread.
pub const PERMISSION_SURFACE_CHAT: &str = "chat";
/// An unattended loop agent: floating prompt plus an inbox row.
pub const PERMISSION_SURFACE_LOOP: &str = "loop";

type Notify = Arc<dyn Fn(&PermissionRequest) + Send + Sync>;
type Resolved = Arc<dyn Fn(&str, &PermissionDecision) + Send + Sync>;

struct Pending {
    req: PermissionRequest,
    done: oneshot::Sender<PermissionDecision>,
}

#[derive(Default)]
struct BrokerState {
    pending: HashMap<String, Pending>,
    /// Tools the human allowed with "remember", per turn (workspace/agent).
    approved: HashMap<String, HashSet<String>>,
    seq: u64,
}

/// Routes permission requests to a human. Every method is safe for
/// concurrent use: requests arrive on MCP tasks, answers on the UI's.
pub struct PermissionBroker {
    state: Mutex<BrokerState>,
    timeout: Mutex<Duration>,
    notify: Mutex<Option<Notify>>,
    resolved: Mutex<Option<Resolved>>,
}

impl Default for PermissionBroker {
    fn default() -> Self {
        Self {
            state: Mutex::default(),
            timeout: Mutex::new(PERMISSION_TIMEOUT),
            notify: Mutex::new(None),
            resolved: Mutex::new(None),
        }
    }
}

fn turn_key(workspace_id: &str, agent_code: &str) -> String {
    format!("{workspace_id}/{agent_code}")
}

impl PermissionBroker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Announces each new request.
    pub fn set_notify(&self, f: impl Fn(&PermissionRequest) + Send + Sync + 'static) {
        *self.notify.lock() = Some(Arc::new(f));
    }

    /// Announces that a request left the queue (answered, expired, abandoned).
    pub fn set_resolved(&self, f: impl Fn(&str, &PermissionDecision) + Send + Sync + 'static) {
        *self.resolved.lock() = Some(Arc::new(f));
    }

    pub fn set_timeout(&self, d: Duration) {
        *self.timeout.lock() = d;
    }

    /// Blocks until the human answers, `cancel` fires (abandoned), or the
    /// timeout denies. An expiry is a verdict, not an error.
    pub async fn request(
        &self,
        cancel: &CancellationToken,
        mut req: PermissionRequest,
    ) -> Result<PermissionDecision, PermissionAbandoned> {
        let key = turn_key(&req.workspace_id, &req.agent_code);
        let rx = {
            let mut st = self.state.lock();
            if st.approved.get(&key).is_some_and(|s| s.contains(&req.tool)) {
                return Ok(PermissionDecision {
                    allow: true,
                    reason: "already approved for this turn".into(),
                    remember: false,
                });
            }
            st.seq += 1;
            req.id = format!("perm-{}-{}", unix_nanos(), st.seq);
            let (tx, rx) = oneshot::channel();
            st.pending.insert(
                req.id.clone(),
                Pending {
                    req: req.clone(),
                    done: tx,
                },
            );
            rx
        };
        let notify = self.notify.lock().clone();
        let timeout = *self.timeout.lock();
        if let Some(f) = notify {
            f(&req);
        }
        tokio::select! {
            d = rx => Ok(d.unwrap_or_default()),
            _ = cancel.cancelled() => {
                self.forget(&req.id);
                self.publish_resolved(&req.id, &PermissionDecision {
                    reason: "the agent stopped before this was answered".into(),
                    ..PermissionDecision::default()
                });
                Err(PermissionAbandoned)
            }
            _ = tokio::time::sleep(timeout) => {
                self.forget(&req.id);
                let d = PermissionDecision {
                    reason: format!(
                        "no answer within {}: denied automatically",
                        recur_project::runconfig::go_duration(timeout)
                    ),
                    ..PermissionDecision::default()
                };
                self.publish_resolved(&req.id, &d);
                Ok(d)
            }
        }
    }

    /// Resolves a pending request. False when it is unknown or already
    /// answered, so a double click is harmless.
    pub fn answer(&self, id: &str, d: PermissionDecision) -> bool {
        let p = {
            let mut st = self.state.lock();
            let Some(p) = st.pending.remove(id) else {
                return false;
            };
            if d.allow && d.remember {
                st.approved
                    .entry(turn_key(&p.req.workspace_id, &p.req.agent_code))
                    .or_default()
                    .insert(p.req.tool.clone());
            }
            p
        };
        let _ = p.done.send(d.clone());
        self.publish_resolved(id, &d);
        true
    }

    /// Unanswered requests ordered by id, so a reloading UI rebuilds its
    /// queue.
    pub fn pending(&self) -> Vec<PermissionRequest> {
        let mut out: Vec<PermissionRequest> = self
            .state
            .lock()
            .pending
            .values()
            .map(|p| p.req.clone())
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    /// Denies everything still pending for a finished turn and drops its
    /// remembered approvals.
    pub fn end_turn(&self, workspace_id: &str, agent_code: &str) {
        let stale: Vec<Pending> = {
            let mut st = self.state.lock();
            st.approved.remove(&turn_key(workspace_id, agent_code));
            let ids: Vec<String> = st
                .pending
                .iter()
                .filter(|(_, p)| {
                    p.req.workspace_id == workspace_id && p.req.agent_code == agent_code
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| st.pending.remove(id)).collect()
        };
        for p in stale {
            let d = PermissionDecision {
                reason: "the turn ended before this was answered".into(),
                ..PermissionDecision::default()
            };
            let _ = p.done.send(d.clone());
            self.publish_resolved(&p.req.id, &d);
        }
    }

    fn forget(&self, id: &str) {
        self.state.lock().pending.remove(id);
    }

    fn publish_resolved(&self, id: &str, d: &PermissionDecision) {
        let f = self.resolved.lock().clone();
        if let Some(f) = f {
            f(id, d);
        }
    }
}

impl Engine {
    /// How the UI should ask: an in-flight chat turn on this tab is watched
    /// by a human (even with a loop running); an unattended loop agent gets
    /// the floating/inbox surface.
    pub fn permission_surface_for(&self, workspace_id: &str, agent_code: &str) -> &'static str {
        if self
            .chats
            .lock()
            .contains_key(&chat_turn_key(workspace_id, agent_code))
        {
            return PERMISSION_SURFACE_CHAT;
        }
        if self.running_loop_by_id(workspace_id).is_some() {
            return PERMISSION_SURFACE_LOOP;
        }
        PERMISSION_SURFACE_CHAT
    }

    /// Host `request_permission`: the engine half of the Claude permission
    /// prompt tool and the compatible harness's Bash gate.
    pub async fn request_permission(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        agent_code: &str,
        ask: PermissionAsk,
    ) -> Result<PermissionVerdict, HostError> {
        let summary = recur_agent::permission::summary(&ask.tool, &ask.input);
        let d = self
            .perms
            .request(
                cancel,
                PermissionRequest {
                    workspace_id: workspace_id.to_owned(),
                    agent_code: agent_code.to_owned(),
                    surface: self
                        .permission_surface_for(workspace_id, agent_code)
                        .to_owned(),
                    tool: ask.tool,
                    input: ask.input,
                    summary,
                    ..PermissionRequest::default()
                },
            )
            .await
            .map_err(|e| HostError(e.to_string()))?;
        Ok(PermissionVerdict {
            allow: d.allow,
            reason: d.reason,
        })
    }

    /// Resolves a pending permission request from the UI. False when it is
    /// already gone (another window, timeout, turn ended).
    pub fn answer_permission(&self, id: &str, d: PermissionDecision) -> bool {
        self.perms.answer(id, d)
    }

    /// Unanswered permission requests (UI reload).
    pub fn pending_permissions(&self) -> Vec<PermissionRequest> {
        self.perms.pending()
    }

    /// The broker (tests shorten its timeout).
    pub fn permission_broker(&self) -> &PermissionBroker {
        &self.perms
    }

    /// `permission.requested`; a loop turn also raises an inbox row and an
    /// OS notification, since nobody may be watching the autopilot.
    pub(crate) fn publish_permission_requested(&self, req: &PermissionRequest) {
        self.bus
            .publish(EngineEvent::PermissionRequested(req.clone()));
        if req.surface != PERMISSION_SURFACE_LOOP {
            return;
        }
        self.store.add_inbox_item(InboxItem {
            id: req.id.clone(),
            group: Group::Needs.as_str().to_owned(),
            kind: "pause".into(),
            title: format!(
                "Permission needed: <b>{}</b>",
                recur_agent::permission::tool_label(&req.tool)
            ),
            sub: req.summary.clone(),
            action: "Answer".into(),
            action_class: "urgent".into(),
            unread: true,
            target: format!("workspace:{}", req.workspace_id),
            workspace_id: req.workspace_id.clone(),
            ..InboxItem::default()
        });
        self.publish_inbox_updated();
        self.publish_notify(
            "Permission needed",
            &req.summary,
            &format!("workspace:{}", req.workspace_id),
        );
        self.publish_resource();
    }

    /// `permission.resolved`: the request left the queue.
    pub(crate) fn publish_permission_resolved(&self, id: &str, d: &PermissionDecision) {
        self.store.remove_inbox_item(id);
        self.bus
            .publish(EngineEvent::PermissionResolved(PermissionResolved {
                allow: d.allow,
                id: id.to_owned(),
                reason: d.reason.clone(),
            }));
        self.publish_inbox_updated();
        self.publish_resource();
    }
}

#[cfg(test)]
mod tests;
