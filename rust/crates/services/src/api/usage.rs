//! Go `internal/usage` types the settings service returns (subscription
//! quota snapshots). They live here until `recur-usage` is ported (slice P3),
//! which should own them; this module then re-exports its types.

use serde::{Deserialize, Serialize};

use super::serde_util::{is_false, is_zero, is_zero_f64};

/// Snapshot is the UI payload for the sidebar footer. Tokens never appear here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Snapshot {
    pub fetched_at: i64,
    pub stale: bool,
    pub providers: Option<Vec<ProviderQuota>>,
}

/// ProviderQuota is one provider's plan windows (and optional Cursor model rows).
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

/// Window is one quota bar (5-hour, 7-day, billing cycle, …).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Window {
    pub id: String,
    pub label: String,
    #[serde(with = "recur_domain::json::float")]
    pub used_pct: f64,
    #[serde(skip_serializing_if = "is_zero")]
    pub resets_at: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

/// ModelRow is one Cursor included-usage model line.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModelRow {
    pub name: String,
    pub tokens: i64,
    #[serde(
        with = "recur_domain::json::float",
        skip_serializing_if = "is_zero_f64"
    )]
    pub used_pct: f64,
    /// Bucket is "cursor" (Composer / Cursor-native) or "other" (API models).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bucket: String,
}

/// CursorUsageConnect is the return of ConnectCursorUsage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CursorUsageConnect {
    pub login_url: String,
    pub pending: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}
