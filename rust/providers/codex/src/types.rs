//! App-server wire types (Go `codex/types.go`). Field order and `omitempty`
//! follow the Go structs so requests encode compatibly with Go's.

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

fn is_false(b: &bool) -> bool {
    !*b
}

/// A durable Codex conversation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thread {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
}

/// One unit of agent work.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Turn {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub status: String,
}

/// The cumulative or last-request footprint from
/// `thread/tokenUsage/updated`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TokenUsage {
    pub total_tokens: i64,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
}

/// One `model/list` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Model {
    pub id: String,
    pub model: String,
    pub display_name: String,
    pub description: String,
    pub hidden: bool,
    pub is_default: bool,
    pub default_reasoning_effort: String,
    #[serde(rename = "supportedReasoningEfforts")]
    pub supported_reasoning_effort: Vec<ReasoningEffort>,
    pub input_modalities: Vec<String>,
    pub additional_speed_tiers: Vec<String>,
    pub service_tiers: Vec<ServiceTier>,
    pub default_service_tier: String,
}

/// One selectable effort level on a model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ReasoningEffort {
    pub reasoning_effort: String,
    pub description: String,
}

/// One speed / priority tier from `model/list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceTier {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// The `account/read` account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plan_type: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountReadResult {
    pub account: Option<Account>,
    pub requires_openai_auth: bool,
}

/// `thread/start` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStartParams {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub approval_policy: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sandbox: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub developer_instructions: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub base_instructions: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub service_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
}

/// One `turn/start` / `turn/steer` input item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UserInput {
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
}

impl UserInput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            typ: "text".into(),
            text: text.into(),
            ..Default::default()
        }
    }
}

/// The turn's sandbox override. `network_access` is omitted when false so
/// the default (blocked) stays off the wire unless Recur opts in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxPolicy {
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(skip_serializing_if = "is_false")]
    pub network_access: bool,
}

/// `turn/start` params.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnStartParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cwd: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub approval_policy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<SandboxPolicy>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub service_tier: String,
    #[serde(rename = "collaborationMode", skip_serializing_if = "Option::is_none")]
    pub collaboration: Option<Box<RawValue>>,
}

/// `turn/steer` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSteerParams {
    pub thread_id: String,
    pub input: Vec<UserInput>,
}

/// `turn/interrupt` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnInterruptParams {
    pub thread_id: String,
    pub turn_id: String,
}

/// Maps Codex `planType` / ChatGPT `plan_type` keys to the CLI's display
/// names; unknown keys pass through trimmed.
pub fn format_plan(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "" => "",
        "free" => "Free",
        "go" => "Go",
        "plus" => "Plus",
        "pro" => "Pro",
        "prolite" => "Pro Lite",
        "team" => "Team",
        "self_serve_business_prolite" => "Self Serve Business ProLite",
        "self_serve_business_usage_based" => "Self Serve Business Usage Based",
        "business" => "Business",
        "ent26" | "enterprise" | "hc" => "Enterprise",
        "enterprise_cbp_automation" => "Enterprise (Automation)",
        "enterprise_cbp_usage_based" => "Enterprise CBP Usage Based",
        "education" | "edu" => "Edu",
        "edu_plus" => "Edu Plus",
        "edu_pro" => "Edu Pro",
        _ => return raw.trim().to_owned(),
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans() {
        for (input, want) in [
            ("", ""),
            ("  ", ""),
            ("free", "Free"),
            ("go", "Go"),
            ("plus", "Plus"),
            ("pro", "Pro"),
            ("PRO", "Pro"),
            ("prolite", "Pro Lite"),
            ("team", "Team"),
            ("self_serve_business_prolite", "Self Serve Business ProLite"),
            (
                "self_serve_business_usage_based",
                "Self Serve Business Usage Based",
            ),
            ("business", "Business"),
            ("ent26", "Enterprise"),
            ("enterprise", "Enterprise"),
            ("hc", "Enterprise"),
            ("enterprise_cbp_automation", "Enterprise (Automation)"),
            ("enterprise_cbp_usage_based", "Enterprise CBP Usage Based"),
            ("edu", "Edu"),
            ("education", "Edu"),
            ("edu_plus", "Edu Plus"),
            ("edu_pro", "Edu Pro"),
            ("mystery_plan", "mystery_plan"),
        ] {
            assert_eq!(format_plan(input), want, "{input:?}");
        }
    }

    #[test]
    fn params_encode_like_go() {
        let p = TurnStartParams {
            thread_id: "t".into(),
            input: vec![UserInput::text("hi")],
            sandbox_policy: Some(SandboxPolicy {
                typ: "workspaceWrite".into(),
                network_access: false,
            }),
            effort: "high".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"threadId":"t","input":[{"type":"text","text":"hi"}],"sandboxPolicy":{"type":"workspaceWrite"},"effort":"high"}"#
        );
        let s = ThreadStartParams {
            model: "m".into(),
            service_name: "recur".into(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"model":"m","serviceName":"recur"}"#
        );
    }
}
