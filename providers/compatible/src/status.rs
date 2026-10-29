//! Settings → Providers Test and flag presets.

use std::collections::HashSet;
use std::time::Duration;

use recur_domain::{CompatibleFlagDim, CompatibleProvider, compatible_flag_preset as preset};
use serde::{Deserialize, Serialize};

use crate::client::Client;

const TEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The Test button outcome for one endpoint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompatibleTestResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

/// POSTs a tiny `/v1/responses` ping with the first model of the saved or
/// draft config. Does not persist anything.
pub async fn test_compatible_provider(p: CompatibleProvider) -> CompatibleTestResult {
    test_with(p, recur_http::default_client()).await
}

pub(crate) async fn test_with(
    p: CompatibleProvider,
    http: reqwest::Client,
) -> CompatibleTestResult {
    let np = match recur_domain::normalize_compatible_provider(p, &HashSet::new()) {
        Ok(np) => np,
        Err(e) => return failed(e.to_string()),
    };
    let model = np.models.first().map(|m| m.id.clone()).unwrap_or_default();
    if model.is_empty() {
        return failed("Add at least one model id first.".into());
    }
    let c = Client::new(&np).with_http(http);
    match tokio::time::timeout(TEST_TIMEOUT, c.ping(&model)).await {
        Err(_) => failed(format!(
            "Post {:?}: context deadline exceeded",
            c.endpoint()
        )),
        Ok(Err(e)) => failed(e),
        Ok(Ok(())) => CompatibleTestResult {
            ok: true,
            error: String::new(),
        },
    }
}

fn failed(error: String) -> CompatibleTestResult {
    CompatibleTestResult { ok: false, error }
}

/// Stock flag dimensions for the Settings editor.
pub fn compatible_flag_preset(name: &str) -> Vec<CompatibleFlagDim> {
    preset(name.trim())
}
