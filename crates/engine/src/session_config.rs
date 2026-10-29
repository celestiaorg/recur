//! A chat/agent session's config JSON (Go `agentConfig` in `mirror.go`):
//! identity, picker state, mode, and the per-chat loop stamp. The store's
//! patch helpers merge keys into it and keep unknown ones.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

fn is_false(b: &bool) -> bool {
    !*b
}

/// Go leaves a map nil on `null`; serde would reject it.
fn null_as_empty<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, String>, D::Error> {
    Ok(Option::<BTreeMap<String, String>>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AgentConfig {
    pub code: String,
    pub name: String,
    /// The chat tab label (PA / PA2 / …).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// A qualified model ref, e.g. `cursor/composer-2.5`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "null_as_empty"
    )]
    pub flags: BTreeMap<String, String>,
    /// The tab's chat mode (ask/plan/agent/review/loop).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    /// The mode whose notes were last attached to the provider session.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub standing_mode: String,
    /// sha256 of the system prompt last attached to a Cursor/Codex session.
    #[serde(rename = "standingPromptSha", skip_serializing_if = "String::is_empty")]
    pub standing_prompt_sha: String,
    /// The composer Simplify chip; `None` = never set (Settings default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub simplify: Option<bool>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub loop_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub policy_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub goal: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub goal_display: String,
    /// The opening paste set for this loop (attachment ids). `Some` (even
    /// empty) means it was recorded; `None` is legacy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub goal_images: Option<Vec<String>>,
    #[serde(skip_serializing_if = "is_false")]
    pub loop_started: bool,
    /// The conductor tab a specialist session belongs to.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub owner_code: String,
    #[serde(skip_serializing_if = "is_false")]
    pub fork_inject: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt_group: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub native_history_after: String,
}

impl AgentConfig {
    /// Go `json.Unmarshal` into a zero config, ignoring errors.
    pub fn parse(config_json: &str) -> Self {
        serde_json::from_str(config_json).unwrap_or_default()
    }
}
