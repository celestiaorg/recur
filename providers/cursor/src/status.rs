//! Settings probes (Go `app/cursor_service.go`). Each spins up a throwaway
//! bridge, independent of the pooled ones, so a key can be validated before
//! it is saved and a bad key never wedges a running bridge.

use std::path::Path;
use std::time::Duration;

use recur_agent::ProviderStatus;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::bridge::{Bridge, BridgeConfig};
use crate::client::Model;
use crate::install::PINNED_SDK_VERSION;
use crate::locate::{owned_bridge_root, sdk_version};

/// Bounds a probe: the first bridge start pays the Node cold start.
const CURSOR_PROBE_TIMEOUT: Duration = Duration::from_secs(120);

/// Recur Connect's session file (Go `usage.cursorAuthFileName`). Its
/// presence gates the status probe: a key without it is a leftover paste.
pub const CURSOR_USAGE_AUTH_FILE: &str = "cursor-usage-auth.json";

/// The Settings card, JSON-identical to Go `CursorStatus`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CursorStatus {
    pub bridge_installed: bool,
    pub bridge_path: String,
    /// The installed cursor-sdk pin from manifest.json.
    pub bridge_version: String,
    /// The release Recur would install.
    pub pinned_version: String,
    /// Recur's own bridge is behind the pin, so Settings offers Reinstall.
    pub update_available: bool,
    pub authenticated: bool,
    /// A short identity for toasts (email or API key name).
    pub account: String,
    #[serde(rename = "apiKeyName")]
    pub api_key_name: String,
    pub email: String,
    pub name: String,
    pub model_count: i64,
    pub error: String,
}

impl From<&ProviderStatus> for CursorStatus {
    fn from(s: &ProviderStatus) -> Self {
        Self {
            bridge_installed: s.installed,
            bridge_path: s.path.clone(),
            bridge_version: s.version.clone(),
            pinned_version: s.pinned_version.clone(),
            update_available: s.update_available,
            authenticated: s.authenticated,
            account: s.account.clone(),
            api_key_name: s.api_key_name.clone(),
            email: s.email.clone(),
            name: s.name.clone(),
            model_count: s.model_count,
            error: s.error.clone(),
        }
    }
}

impl From<CursorStatus> for ProviderStatus {
    fn from(s: CursorStatus) -> Self {
        Self {
            installed: s.bridge_installed,
            path: s.bridge_path,
            version: s.bridge_version,
            pinned_version: s.pinned_version,
            update_available: s.update_available,
            authenticated: s.authenticated,
            account: s.account,
            api_key_name: s.api_key_name,
            email: s.email,
            name: s.name,
            model_count: s.model_count,
            error: s.error,
            ..Default::default()
        }
    }
}

/// One throwaway bridge round trip.
#[derive(Debug, Clone, Default)]
pub(crate) struct Probe {
    pub bridge_version: String,
    pub account: String,
    pub api_key_name: String,
    pub email: String,
    pub name: String,
    pub model_count: i64,
    pub models: Vec<Model>,
}

/// Whether Recur's Connect session file exists (stat only).
pub(crate) fn has_recur_cursor_auth_file(data_dir: &Path) -> bool {
    !data_dir.as_os_str().is_empty()
        && std::fs::metadata(data_dir.join(CURSOR_USAGE_AUTH_FILE)).is_ok()
}

/// The pin, and whether Recur's own install is behind it. Bridges outside
/// the data dir never offer an update.
pub(crate) fn apply_cursor_pin(out: &mut CursorStatus, data_dir: &Path, bin: &Path) {
    out.pinned_version = PINNED_SDK_VERSION.to_owned();
    if owned_bridge_root(data_dir, bin).is_none() {
        out.update_available = false;
        return;
    }
    out.update_available =
        !out.bridge_version.is_empty() && out.bridge_version != out.pinned_version;
}

pub(crate) fn apply_cursor_probe(out: &mut CursorStatus, p: &Probe) {
    out.bridge_version = p.bridge_version.clone();
    out.authenticated = true;
    out.account = p.account.clone();
    out.api_key_name = p.api_key_name.clone();
    out.email = p.email.clone();
    out.name = p.name.clone();
    out.model_count = p.model_count;
}

/// Starts a temporary bridge (no log dir, so it never reaps the pooled
/// ones) and exercises the credentialed path.
pub(crate) async fn probe_cursor(bin: &Path, api_key: &str) -> Result<Probe, String> {
    let cancel = CancellationToken::new();
    match tokio::time::timeout(CURSOR_PROBE_TIMEOUT, probe_inner(bin, api_key, &cancel)).await {
        Ok(r) => r,
        Err(_) => {
            cancel.cancel();
            Err("context deadline exceeded".into())
        }
    }
}

async fn probe_inner(
    bin: &Path,
    api_key: &str,
    cancel: &CancellationToken,
) -> Result<Probe, String> {
    let b = Bridge::start(
        BridgeConfig {
            bin: bin.to_path_buf(),
            api_key: api_key.to_owned(),
            ..Default::default()
        },
        cancel,
    )
    .await
    .map_err(|e| format!("could not start the Cursor bridge: {e}"))?;
    let res = probe_bridge(&b, bin, api_key, cancel).await;
    b.close().await;
    res
}

async fn probe_bridge(
    b: &Bridge,
    bin: &Path,
    api_key: &str,
    cancel: &CancellationToken,
) -> Result<Probe, String> {
    let Some(cl) = b.connect_client() else {
        return Err("bridge handshake failed: bridge not started".into());
    };
    cl.get_version(cancel)
        .await
        .map_err(|e| format!("bridge handshake failed: {e}"))?;
    // ListModels is the cheapest credentialed call: the key check.
    let models = cl.list_models(cancel, api_key).await.map_err(|e| {
        if e.connect().is_some_and(|c| c.unauthenticated()) {
            "that API key was rejected by Cursor".to_owned()
        } else {
            e.to_string()
        }
    })?;
    // The manifest pin, not GetVersion's shell version.
    let mut out = Probe {
        bridge_version: sdk_version(bin),
        model_count: models.len() as i64,
        models,
        ..Default::default()
    };
    // Identity is a nice-to-have: a key that lists models is valid.
    if let Ok(me) = cl.me(cancel, api_key).await {
        out.api_key_name = me.api_key_name.clone();
        out.email = me.user_email.clone();
        out.name = me.display_name();
        out.account = me.summary();
    }
    Ok(out)
}

/// The models a key may use through a throwaway bridge.
pub(crate) async fn list_models(bin: &Path, api_key: &str) -> Result<Vec<Model>, String> {
    let cancel = CancellationToken::new();
    let run = async {
        let b = Bridge::start(
            BridgeConfig {
                bin: bin.to_path_buf(),
                api_key: api_key.to_owned(),
                ..Default::default()
            },
            &cancel,
        )
        .await
        .map_err(|e| e.to_string())?;
        let res = match b.connect_client() {
            Some(cl) => cl
                .list_models(&cancel, api_key)
                .await
                .map_err(|e| e.to_string()),
            None => Err("bridge not started".into()),
        };
        b.close().await;
        res
    };
    match tokio::time::timeout(CURSOR_PROBE_TIMEOUT, run).await {
        Ok(r) => r,
        Err(_) => {
            cancel.cancel();
            Err("context deadline exceeded".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_exec(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        recur_exec::write_executable(p, "#!/bin/sh\n", 0o755).unwrap();
    }

    // Go TestApplyCursorPin.
    #[test]
    fn apply_pin() {
        let data = tempfile::tempdir().unwrap();
        let owned = data.path().join("cursor-bridge/bin/cursor-sdk-bridge");
        write_exec(&owned);

        let mut out = CursorStatus {
            bridge_version: "1.0.27".into(),
            ..Default::default()
        };
        apply_cursor_pin(&mut out, data.path(), &owned);
        assert_eq!(out.pinned_version, PINNED_SDK_VERSION);
        assert!(out.update_available);

        let mut out = CursorStatus {
            bridge_version: PINNED_SDK_VERSION.into(),
            ..Default::default()
        };
        apply_cursor_pin(&mut out, data.path(), &owned);
        assert!(!out.update_available);

        let mut out = CursorStatus::default();
        apply_cursor_pin(&mut out, data.path(), &owned);
        assert!(!out.update_available);

        let other = tempfile::tempdir().unwrap();
        let external = other.path().join("cursor-bridge/bin/cursor-sdk-bridge");
        write_exec(&external);
        let mut out = CursorStatus {
            bridge_version: "1.0.27".into(),
            ..Default::default()
        };
        apply_cursor_pin(&mut out, data.path(), &external);
        assert!(!out.update_available);
        assert_eq!(out.pinned_version, PINNED_SDK_VERSION);
    }

    #[test]
    fn status_json_matches_go_tags() {
        let s = CursorStatus {
            bridge_installed: true,
            api_key_name: "k".into(),
            model_count: 3,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&s).unwrap(),
            r#"{"bridgeInstalled":true,"bridgePath":"","bridgeVersion":"","pinnedVersion":"","updateAvailable":false,"authenticated":false,"account":"","apiKeyName":"k","email":"","name":"","modelCount":3,"error":""}"#
        );
        assert_eq!(CursorStatus::from(&ProviderStatus::from(s.clone())), s);
    }
}
