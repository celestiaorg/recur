//! Questions an agent parks for the human: loop escalations, ask_user
//! prompts, and live tool permission requests.

use serde::{Deserialize, Serialize};

use crate::serde_util::is_false;

/// One reasoned choice the Maestro offers (PRD §13).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EscalationOption {
    pub text: String,
    /// simplest / smoothest / recommended.
    pub tag: String,
    pub desc: String,
}

/// A scoped question the loop pauses on (PRD §13).
///
/// Decoding treats a missing (or null) `applied` on an answered row as
/// already consumed: pre-Applied rows were consumed in-process, and replaying
/// them after upgrade would re-steer or re-grant.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "EscalationWire", rename_all = "camelCase")]
pub struct Escalation {
    pub id: String,
    pub workspace_id: String,
    /// The owning chat/walk; empty = workspace-scoped.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent_code: String,
    pub question: String,
    pub context: String,
    /// `None` serializes as `null`.
    pub options: Option<Vec<EscalationOption>>,
    /// The single-line resolved value (free text wins, else the choice).
    pub answer: String,
    /// The structured option text when the human picked one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub choice: String,
    /// The freeform reply.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub free_text: String,
    /// open/answered.
    pub status: String,
    /// Engine-stamped provenance for restart dispatch (agent/cap/ensure_pr/
    /// plan). Agents cannot set this.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// A loop has consumed the answer. Answered but not applied is the
    /// durable pending-answer state for a delayed dispatch.
    pub applied: bool,
    pub asked_ago: String,
}

#[derive(Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct EscalationWire {
    id: String,
    workspace_id: String,
    agent_code: String,
    question: String,
    context: String,
    options: Option<Vec<EscalationOption>>,
    answer: String,
    choice: String,
    free_text: String,
    status: String,
    kind: String,
    applied: Option<bool>,
    asked_ago: String,
}

impl From<EscalationWire> for Escalation {
    fn from(w: EscalationWire) -> Self {
        let applied = w.applied.unwrap_or(w.status == "answered");
        Self {
            id: w.id,
            workspace_id: w.workspace_id,
            agent_code: w.agent_code,
            question: w.question,
            context: w.context,
            options: w.options,
            answer: w.answer,
            choice: w.choice,
            free_text: w.free_text,
            status: w.status,
            kind: w.kind,
            applied,
            asked_ago: w.asked_ago,
        }
    }
}

/// One parked ask_user prompt from an interactive agent. The calling chat
/// turn ends; answering starts a new resumed turn. Not durable across
/// restarts, and unlike Escalation it does not escalate the workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UserQuestion {
    pub id: String,
    pub workspace_id: String,
    pub agent_code: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub question: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub context: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<EscalationOption>,
    pub asked_ago: String,
}

/// The human's reply to a UserQuestion.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UserQuestionAnswer {
    /// The selected option text (or id) when the human picked one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub choice: String,
    /// The freeform answer (card input or chat composer send).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub free_text: String,
}

/// One live "may I run this tool?" question, raised when bypass is off. Not
/// persisted: the agent holds an open MCP request while it waits, so a
/// request cannot outlive the turn that raised it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PermissionRequest {
    pub id: String,
    pub workspace_id: String,
    pub agent_code: String,
    /// "loop" for unattended autopilot agents (also raises an inbox row), or
    /// "chat" for an in-flight PA*/MA chat turn (rendered inline).
    pub surface: String,
    /// The tool being requested, e.g. "Bash" or "Write".
    pub tool: String,
    /// The tool's arguments as raw JSON.
    pub input: String,
    /// A one-line human rendering of Tool+Input.
    pub summary: String,
    pub asked_ago: String,
}

/// The human's answer to a PermissionRequest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionDecision {
    /// A denial carries Reason back to the model so it can adapt.
    pub allow: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// On an allow, approve the tool for the rest of the turn.
    #[serde(skip_serializing_if = "is_false")]
    pub remember: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_legacy_json() {
        let e: Escalation =
            serde_json::from_str(r#"{"workspaceId":"W","status":"answered","answer":"ok"}"#)
                .unwrap();
        assert!(
            e.applied,
            "legacy answered row with no applied field decodes as applied"
        );

        let e: Escalation = serde_json::from_str(
            r#"{"workspaceId":"W","status":"answered","answer":"ok","applied":false}"#,
        )
        .unwrap();
        assert!(!e.applied, "explicit applied:false is still pending");

        let e: Escalation = serde_json::from_str(r#"{"workspaceId":"W","status":"open"}"#).unwrap();
        assert!(!e.applied, "open row without applied stays false");

        let e: Escalation =
            serde_json::from_str(r#"{"workspaceId":"W","status":"answered","applied":null}"#)
                .unwrap();
        assert!(
            e.applied,
            "null applied on an answered row is treated as missing"
        );
    }

    #[test]
    fn round_trip() {
        let e = Escalation {
            id: "E1".into(),
            status: "open".into(),
            ..Default::default()
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "id": "E1", "workspaceId": "", "question": "", "context": "",
                "options": null, "answer": "", "status": "open", "applied": false,
                "askedAgo": ""
            })
        );
        assert_eq!(serde_json::from_value::<Escalation>(v).unwrap(), e);
    }
}
