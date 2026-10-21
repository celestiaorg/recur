//! The global policy surface and the actionable inbox.

use serde::{Deserialize, Serialize};

use crate::AutonomyLevel;

/// One hard guardrail (PRD §9). Toggleable in Policy; enforced by the engine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rail {
    pub key: String,
    pub title: String,
    pub desc: String,
    pub on: bool,
}

/// The single global control surface (PRD §9, §18), one row in v1.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub autonomy: AutonomyLevel,
    /// `None` is Go's nil slice (`null`).
    pub rails: Option<Vec<Rail>>,
}

/// An actionable "what needs me" row (PRD §16.11, D5 actionable-only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InboxItem {
    pub id: String,
    /// needs/ready/upd.
    pub group: String,
    /// pause/pr/loop/merge (icon).
    pub kind: String,
    pub color: String,
    /// May contain <b>.
    pub title: String,
    pub sub: String,
    pub time: String,
    /// Answer/Review/View.
    pub action: String,
    /// urgent/go/"".
    pub action_class: String,
    pub unread: bool,
    /// Route target, e.g. "loop:LOOP-2218" / "pr:482" / "view:pr".
    pub target: String,
    /// Joins the row onto its Briefing workspace card ("" = app-level).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub workspace_id: String,
}
