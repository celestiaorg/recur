//! Settings → Providers Test and flag presets.

use std::collections::HashSet;
use std::time::Duration;

use recur_agent::catalog::HarnessChoice;
use recur_domain::{
    CompatibleApi, CompatibleFlagDim, CompatibleProvider, compatible_flag_preset as preset,
};
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
    /// The wire APIs that answered. Unknown when neither did.
    #[serde(skip_serializing_if = "CompatibleApi::is_unknown")]
    pub api: CompatibleApi,
    /// The harnesses that can run this endpoint's models, unavailable ones
    /// with a reason. Services fill it in; empty when the test failed.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<HarnessChoice>,
}

impl CompatibleTestResult {
    /// The Settings line: which API answered and where models can run.
    pub fn summary(&self) -> String {
        if !self.ok {
            return if self.error.is_empty() {
                "Could not reach endpoint".into()
            } else {
                self.error.clone()
            };
        }
        let mut out = format!("Connected with {}.", self.api.label());
        let ready: Vec<&str> = self
            .harnesses
            .iter()
            .filter(|h| !h.unavailable)
            .map(|h| h.label.as_str())
            .collect();
        if !ready.is_empty() {
            out.push_str(&format!(" Runs in {}.", ready.join(" and ")));
        }
        for h in self.harnesses.iter().filter(|h| h.unavailable) {
            out.push_str(&format!(" {} unavailable: {}", h.label, h.reason));
            if !h.reason.ends_with('.') {
                out.push('.');
            }
        }
        out
    }
}

/// POSTs a tiny `/v1/responses` and `/v1/chat/completions` ping with the
/// first model of the saved or draft config and reports which answered.
/// Does not persist anything.
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
    let timed = |url: String, r: Option<Result<(), String>>| match r {
        Some(r) => r,
        None => Err(format!("Post {url:?}: context deadline exceeded")),
    };
    let (responses, chat) = tokio::join!(
        tokio::time::timeout(TEST_TIMEOUT, c.ping(&model)),
        tokio::time::timeout(TEST_TIMEOUT, c.ping_chat(&model)),
    );
    let responses = timed(c.endpoint(), responses.ok());
    let chat = timed(c.chat_endpoint(), chat.ok());
    let api = CompatibleApi::from_probes(responses.is_ok(), chat.is_ok());
    match (responses, chat) {
        (Err(r), Err(c)) if r == c => failed(r),
        (Err(r), Err(c)) => failed(format!("Responses: {r}. Chat Completions: {c}")),
        _ => CompatibleTestResult {
            ok: true,
            api,
            ..Default::default()
        },
    }
}

fn failed(error: String) -> CompatibleTestResult {
    CompatibleTestResult {
        ok: false,
        error,
        ..Default::default()
    }
}

/// Stock flag dimensions for the Settings editor.
pub fn compatible_flag_preset(name: &str) -> Vec<CompatibleFlagDim> {
    preset(name.trim())
}
