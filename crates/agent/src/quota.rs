//! Subscription quota snapshot types, and the probe providers expose for the
//! sidebar footer.

use serde::{Deserialize, Serialize};

pub const PROVIDER_CLAUDE: &str = "claude-code";
pub const PROVIDER_CURSOR: &str = "cursor";
pub const PROVIDER_CODEX: &str = "codex";

pub const SOURCE_CLAUDE_LOGIN: &str = "Claude Code login";
pub const SOURCE_CODEX_CHATGPT: &str = "ChatGPT login";
pub const SOURCE_CODEX_API_KEY: &str = "API key";
pub const SOURCE_CURSOR_CONNECT: &str = "Recur Connect";

pub const SOURCE_ID_CLAUDE_LOGIN: &str = "claude_login";
pub const SOURCE_ID_CODEX_CHATGPT: &str = "chatgpt";
pub const SOURCE_ID_CODEX_API_KEY: &str = "api_key";
pub const SOURCE_ID_CURSOR_CONNECT: &str = "cursor_connect";

pub const WINDOW_FIVE_HOUR: &str = "five_hour";
pub const WINDOW_SEVEN_DAY: &str = "seven_day";
pub const WINDOW_EXTRA: &str = "extra";
pub const WINDOW_OPUS: &str = "seven_day_opus";
pub const WINDOW_SONNET: &str = "seven_day_sonnet";
pub const WINDOW_FABLE: &str = "seven_day_fable";
pub const WINDOW_BILLING: &str = "billing";

pub const BUCKET_CURSOR: &str = "cursor";
pub const BUCKET_OTHER: &str = "other";

/// The sidebar footer payload. Tokens never appear here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Snapshot {
    pub fetched_at: i64,
    pub stale: bool,
    /// `None` serializes as `null`; [`empty_snapshot`] has `[]`.
    pub providers: Option<Vec<ProviderQuota>>,
}

/// One provider's plan windows (and optional Cursor model rows).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderQuota {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub email: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub plan: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hint: String,
    #[serde(skip_serializing_if = "is_false")]
    pub connecting: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<Window>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelRow>,
}

/// One quota bar (5-hour, 7-day, billing cycle, …).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Window {
    pub id: String,
    pub label: String,
    pub used_pct: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub resets_at: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// One Cursor included-usage model line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelRow {
    pub name: String,
    pub tokens: i64,
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub used_pct: f64,
    /// `cursor` (Composer / Cursor-native) or `other` (API models).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bucket: String,
}

/// The result of starting Cursor usage Connect.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CursorUsageConnect {
    pub login_url: String,
    pub pending: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

fn is_zero_f64(n: &f64) -> bool {
    *n == 0.0
}

/// A zero snapshot for first paint (`providers: []`).
pub fn empty_snapshot() -> Snapshot {
    Snapshot {
        providers: Some(Vec::new()),
        ..Default::default()
    }
}

/// Sets the display source and its stable id.
pub fn set_source(q: &mut ProviderQuota, display: &str) {
    q.source = display.to_owned();
    q.source_id = match display {
        SOURCE_CLAUDE_LOGIN => SOURCE_ID_CLAUDE_LOGIN,
        SOURCE_CODEX_CHATGPT => SOURCE_ID_CODEX_CHATGPT,
        SOURCE_CODEX_API_KEY => SOURCE_ID_CODEX_API_KEY,
        SOURCE_CURSOR_CONNECT => SOURCE_ID_CURSOR_CONNECT,
        _ => "",
    }
    .to_owned();
}

/// Clamps a percentage to 0..=100.
pub fn clamp_pct(n: f64) -> f64 {
    n.clamp(0.0, 100.0)
}

/// A provider's subscription quota source. Errors land in
/// `ProviderQuota.error` / `.hint`, never as a failure.
#[async_trait::async_trait]
pub trait UsageProbe: Send + Sync {
    fn provider_id(&self) -> &str;
    async fn fetch(&self) -> ProviderQuota;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_matches_go() {
        assert_eq!(
            serde_json::to_string(&empty_snapshot()).unwrap(),
            r#"{"fetchedAt":0,"stale":false,"providers":[]}"#
        );
        let mut q = ProviderQuota {
            id: PROVIDER_CURSOR.into(),
            name: "Cursor".into(),
            windows: vec![Window {
                id: WINDOW_BILLING.into(),
                label: "Billing".into(),
                used_pct: 50.0,
                ..Default::default()
            }],
            models: vec![ModelRow {
                name: "composer".into(),
                tokens: 10,
                used_pct: 2.5,
                bucket: BUCKET_CURSOR.into(),
            }],
            ..Default::default()
        };
        set_source(&mut q, SOURCE_CURSOR_CONNECT);
        assert_eq!(
            serde_json::to_string(&q).unwrap(),
            r#"{"id":"cursor","name":"Cursor","source":"Recur Connect","sourceId":"cursor_connect","windows":[{"id":"billing","label":"Billing","usedPct":50.0}],"models":[{"name":"composer","tokens":10,"usedPct":2.5,"bucket":"cursor"}]}"#
        );
        set_source(&mut q, "other");
        assert_eq!(q.source_id, "");
    }

    #[test]
    fn clamps() {
        assert_eq!(clamp_pct(-1.0), 0.0);
        assert_eq!(clamp_pct(101.0), 100.0);
        assert_eq!(clamp_pct(42.5), 42.5);
    }
}
